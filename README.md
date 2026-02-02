# acteon

Actions forged in Rust

Acteon is an action gateway that dispatches actions through a configurable pipeline of rules, providers, and state backends.

## Architecture

| Crate | Description |
|-------|-------------|
| `acteon-core` | Shared types (`Action`, `ActionOutcome`, newtypes) |
| `acteon-state` | Abstract state store / distributed lock trait |
| `acteon-state-memory` | In-memory state backend |
| `acteon-state-redis` | Redis state backend |
| `acteon-state-postgres` | PostgreSQL state backend |
| `acteon-state-dynamodb` | DynamoDB state backend |
| `acteon-state-etcd` | etcd state backend |
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

- Rust 1.75+
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
backend = "memory"   # "memory", "redis", "postgres", "dynamodb", or "etcd"
# url = "redis://localhost:6379"
# prefix = "acteon"
# region = "us-east-1"       # DynamoDB only
# table_name = "acteon"      # DynamoDB only

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

MIT
