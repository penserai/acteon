<p align="center">
  <img src="docs/logo.svg" alt="Acteon — Actions forged in Rust" width="200">
</p>

<p align="center">
  <a href="https://github.com/penserai/acteon/actions/workflows/ci.yml"><img src="https://github.com/penserai/acteon/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

Acteon is an action gateway that dispatches actions through a configurable pipeline of rules, providers, and state backends.

The name draws from the Greek myth of Actaeon, a hunter transformed by Artemis into a stag -- the very thing he pursued. Likewise, actions entering Acteon are transformed -- deduplicated, rerouted, throttled, or dispatched -- before they ever reach the outside world.

## Guides

> **[AI Agent Swarm Coordination](https://penserai.github.io/acteon/guides/agent-swarm-coordination/)** — Use Acteon as a safety and orchestration layer for multi-agent AI systems. Covers identity isolation, permission control, prompt injection defense, rate limiting, approval workflows, failure isolation, and full observability.

## Features

### Rule-Based Action Processing

- **Suppression** — Block actions matching specific conditions (e.g., spam filtering, maintenance windows)
- **Deduplication** — Prevent duplicate processing using configurable keys and TTLs
- **Throttling** — Rate-limit actions per tenant, provider, or action type with automatic retry-after hints
- **Rerouting** — Dynamically redirect actions to different providers based on priority, load, or content
- **Payload Modification** — Transform action payloads before execution (redaction, enrichment, normalization)

### Event Grouping & State Machines

- **Event Grouping** — Batch related events together for consolidated notifications with configurable wait times and group sizes
- **State Machines** — Track event lifecycle through configurable states (e.g., open → investigating → resolved) with automatic timeout transitions
- **Inhibition** — Suppress dependent events when parent events are active using expression functions
- **Fingerprinting** — Correlate related events using configurable field-based fingerprints
- **Background Processing** — Automatic group flushing, timeout processing, and state cleanup

### Pluggable Backends

- **State Storage** — Memory, Redis, PostgreSQL, DynamoDB, or ClickHouse for distributed locks and deduplication state
- **Audit Trail** — Memory, PostgreSQL, ClickHouse, or Elasticsearch for searchable action history with configurable retention

### Enterprise Ready

- **Multi-Tenant** — Namespace and tenant isolation with per-tenant rate limiting
- **Authentication** — API key and JWT support with role-based access control and grant-level authorization
- **Hot Reload** — Update rules and auth configuration without restarts
- **Graceful Shutdown** — Drain in-flight requests before stopping
- **Observability** — Prometheus metrics, structured logging, and comprehensive audit trails

### Developer Experience

- **Admin UI** — Polished web interface for monitoring and configuration
- **OpenAPI/Swagger** — Auto-generated API documentation with interactive UI
- **Polyglot Clients** — Official SDKs for Rust, Python, Node.js/TypeScript, Go, and Java
- **Simulation Framework** — Test harness with mock providers, failure injection, and multi-node scenarios
- **YAML Rules** — Human-readable rule definitions with CEL expression support

## Architecture

All crates are organized under `crates/` with logical groupings:

### Core Components

| Crate | Description |
|-------|-------------|
| `crates/core` | Shared types (`Action`, `ActionOutcome`, newtypes) |
| [`crates/client`](crates/client/README.md) | Native Rust HTTP client for the Acteon API |
| `crates/server` | HTTP server (Axum) with Swagger UI |
| `crates/gateway` | Orchestrates lock, rules, execution, grouping, and state machines |
| [`crates/simulation`](crates/simulation/README.md) | Testing framework with mock providers and failure injection |
| `crates/executor` | Action execution with retries and concurrency |
| `crates/provider` | Provider trait and registry |

### State Backends

| Crate | Description |
|-------|-------------|
| `crates/state/state` | Abstract state store / distributed lock trait |
| `crates/state/memory` | In-memory state backend |
| `crates/state/redis` | Redis state backend |
| `crates/state/postgres` | PostgreSQL state backend |
| `crates/state/dynamodb` | DynamoDB state backend |
| `crates/state/clickhouse` | ClickHouse state backend |

### Audit Backends

| Crate | Description |
|-------|-------------|
| `crates/audit/audit` | Abstract audit trail trait |
| `crates/audit/memory` | In-memory audit backend |
| `crates/audit/postgres` | PostgreSQL audit backend |
| `crates/audit/clickhouse` | ClickHouse audit backend |
| `crates/audit/elasticsearch` | Elasticsearch audit backend |

### Rules Frontends

| Crate | Description |
|-------|-------------|
| `crates/rules/rules` | Rule engine IR and evaluation |
| `crates/rules/yaml` | YAML rule file parser |
| `crates/rules/cel` | CEL expression support |

### Integrations

| Crate | Description |
|-------|-------------|
| `crates/integrations/email` | Email/SMTP provider |
| `crates/integrations/slack` | Slack provider |
| `crates/integrations/pagerduty` | PagerDuty Events API v2 provider |

## Running locally

### Prerequisites

- Rust 1.88+
- Cargo

### Quick start (in-memory, no config file needed)

```sh
cargo run -p acteon-server
```

The server starts on `http://127.0.0.1:8080` with the in-memory state backend and no rules loaded. You can then:

- Open the **Admin UI** at [http://127.0.0.1:8080/](http://127.0.0.1:8080/)
- Open **Swagger UI** at [http://127.0.0.1:8080/swagger-ui/](http://127.0.0.1:8080/swagger-ui/)
- Fetch the **OpenAPI spec** at [http://127.0.0.1:8080/api-doc/openapi.json](http://127.0.0.1:8080/api-doc/openapi.json)
- Hit the **health endpoint**: `curl http://127.0.0.1:8080/health`

### CLI options

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

Examples:

```sh
# Custom port
cargo run -p acteon-server -- --port 3000

# With a config file
cargo run -p acteon-server -- -c my-config.toml

# Run database migrations before first start
scripts/migrate.sh -c my-config.toml
```

### Configuration

Create an `acteon.toml` file (all sections are optional -- defaults are shown):

```toml
[server]
host = "127.0.0.1"
port = 8080
# shutdown_timeout_seconds = 30  # Max time to wait for pending tasks during shutdown

[ui]
# enabled = true
# dist_path = "ui/dist"

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
scripts/migrate.sh -c examples/postgres.toml
cargo run -p acteon-server --features postgres -- -c examples/postgres.toml

# ClickHouse state + audit
docker compose --profile clickhouse up -d
scripts/migrate.sh -c examples/clickhouse.toml
cargo run -p acteon-server --features clickhouse -- -c examples/clickhouse.toml

# Redis state + Elasticsearch audit
docker compose --profile elasticsearch up -d
scripts/migrate.sh -c examples/elasticsearch-audit.toml
cargo run -p acteon-server -- -c examples/elasticsearch-audit.toml

# DynamoDB Local state
docker compose --profile dynamodb up -d
scripts/migrate.sh -c examples/dynamodb.toml
cargo run -p acteon-server --features dynamodb -- -c examples/dynamodb.toml
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
scripts/migrate.sh -c acteon.toml
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
| GET | `/v1/events` | List events filtered by status |
| GET | `/v1/events/{fingerprint}` | Get event lifecycle state |
| PUT | `/v1/events/{fingerprint}/transition` | Transition event to new state |
| GET | `/v1/groups` | List active event groups |
| GET | `/v1/groups/{group_key}` | Get group details |
| DELETE | `/v1/groups/{group_key}` | Force flush/close a group |

Full request/response schemas are available in the Swagger UI.

## Event Grouping & State Machines

### State Machine Rules

Track event lifecycle through configurable states with automatic timeout transitions:

```yaml
# rules/alert-lifecycle.yaml
rules:
  - name: alert-state-machine
    condition:
      field: action.action_type
      eq: alert
    action:
      type: state_machine
      state_machine: alert
      fingerprint_fields:
        - action_type
        - metadata.cluster
        - metadata.service
```

Configure state machines in your `acteon.toml`:

```toml
[[state_machines]]
name = "alert"
initial_state = "firing"
states = ["firing", "acknowledged", "resolved"]

[[state_machines.transitions]]
from = "firing"
to = "acknowledged"

[[state_machines.transitions]]
from = "acknowledged"
to = "resolved"

[[state_machines.timeouts]]
state = "firing"
after_seconds = 3600
transition_to = "stale"
```

### Event Grouping

Batch related events for consolidated notifications:

```yaml
rules:
  - name: group-cluster-alerts
    condition:
      field: action.action_type
      starts_with: cluster_
    action:
      type: group
      group_by:
        - metadata.cluster
        - metadata.severity
      group_wait_seconds: 60      # Wait before first notification
      group_interval_seconds: 300 # Min time between notifications
      max_group_size: 100
```

### Inhibition

Suppress dependent events when parent events are active using expression functions:

```yaml
rules:
  - name: inhibit-pod-alerts-on-cluster-down
    condition:
      all:
        - field: action.action_type
          starts_with: pod_
        - call: has_active_event
          args: [cluster_down, action.metadata.cluster]
    action:
      type: suppress
      reason: "Cluster is down"
```

Available expression functions for state lookups:
- `has_active_event(event_type, label_value)` — Check if an active event exists
- `get_event_state(fingerprint)` — Get current state of an event
- `event_in_state(fingerprint, state)` — Check if event is in a specific state

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

Official client libraries are available for multiple languages:

| Language | Package | Documentation |
|----------|---------|---------------|
| Rust | `acteon-client` | [README](acteon-client/README.md) |
| Python | `acteon-client` | [README](clients/python/README.md) |
| Node.js/TypeScript | `@acteon/client` | [README](clients/nodejs/README.md) |
| Go | `github.com/penserai/acteon/clients/go/acteon` | [README](clients/go/README.md) |
| Java | `com.acteon:acteon-client` | [README](clients/java/README.md) |

### Quick Examples

**Rust:**
```rust
let client = ActeonClient::new("http://localhost:8080");
let action = Action::new("ns", "tenant", "email", "send", json!({"to": "user@example.com"}));
let outcome = client.dispatch(&action).await?;
```

**Python:**
```python
client = ActeonClient("http://localhost:8080")
action = Action("ns", "tenant", "email", "send", {"to": "user@example.com"})
outcome = client.dispatch(action)
```

**Node.js/TypeScript:**
```typescript
const client = new ActeonClient("http://localhost:8080");
const action = createAction("ns", "tenant", "email", "send", { to: "user@example.com" });
const outcome = await client.dispatch(action);
```

**Go:**
```go
client := acteon.NewClient("http://localhost:8080")
action := acteon.NewAction("ns", "tenant", "email", "send", map[string]any{"to": "user@example.com"})
outcome, _ := client.Dispatch(ctx, action)
```

**Java:**
```java
ActeonClient client = new ActeonClient("http://localhost:8080");
Action action = new Action("ns", "tenant", "email", "send", Map.of("to", "user@example.com"));
ActionOutcome outcome = client.dispatch(action);
```

See the [clients directory](clients/README.md) for full documentation.

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
