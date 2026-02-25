# Installation

## Building from Source

Clone the repository and build:

```bash
git clone https://github.com/penserai/acteon.git
cd acteon
cargo build --workspace
```

### Feature Flags

The server supports multiple backends via feature flags. By default, Redis is enabled:

```bash
# Default build (includes Redis)
cargo build -p acteon-server

# All backends
cargo build -p acteon-server --features all-backends

# Specific backends
cargo build -p acteon-server --features "postgres,elasticsearch"
```

| Feature | Description |
|---------|-------------|
| `redis` | Redis state backend (default) |
| `postgres` | PostgreSQL state and audit backends |
| `clickhouse` | ClickHouse state and audit backends |
| `dynamodb` | AWS DynamoDB state and audit backends |
| `elasticsearch` | Elasticsearch audit backend |
| `all-backends` | All backends enabled |

#### AWS Provider Features

AWS providers are **not compiled by default**. Each provider has its own feature flag, so you only pay the compile cost for the SDKs you actually use:

```bash
# Single AWS provider
cargo build -p acteon-server --features aws-sns

# A few providers
cargo build -p acteon-server --features "aws-sns,aws-lambda,aws-s3"

# All AWS providers
cargo build -p acteon-server --features aws-all
```

| Feature | AWS Service |
|---------|------------|
| `aws-sns` | Simple Notification Service |
| `aws-lambda` | Lambda |
| `aws-eventbridge` | EventBridge |
| `aws-sqs` | Simple Queue Service |
| `aws-ses` | Simple Email Service |
| `aws-s3` | Simple Storage Service |
| `aws-ec2` | EC2 |
| `aws-autoscaling` | Auto Scaling |
| `aws-all` | All eight AWS providers |

### Running the Server

```bash
# Quick start with in-memory backend (no dependencies)
cargo run -p acteon-server

# With a config file
cargo run -p acteon-server -- -c acteon.toml

# Custom host and port
cargo run -p acteon-server -- --host 0.0.0.0 --port 3000
```

## Docker

Build and run with Docker:

```bash
docker build -t acteon .
docker run -p 8080:8080 acteon
```

## Setting Up Backends with Docker Compose

The project includes a `docker-compose.yml` with profiles for every backend:

```bash
# Start Redis (always runs by default)
docker compose up -d

# Start PostgreSQL
docker compose --profile postgres up -d

# Start ClickHouse
docker compose --profile clickhouse up -d

# Start Elasticsearch
docker compose --profile elasticsearch up -d

# Start DynamoDB Local
docker compose --profile dynamodb up -d

# Start multiple backends
docker compose --profile postgres --profile elasticsearch up -d
```

### Backend Connection Defaults

| Backend | Docker Profile | Default URL |
|---------|---------------|-------------|
| Memory | _(none)_ | n/a |
| Redis | _(default)_ | `redis://localhost:6379` |
| PostgreSQL | `postgres` | `postgres://acteon:acteon@localhost:5432/acteon` |
| ClickHouse | `clickhouse` | `http://localhost:8123` |
| Elasticsearch | `elasticsearch` | `http://localhost:9200` |
| DynamoDB Local | `dynamodb` | `http://localhost:8000` |

## Verifying the Installation

Once the server is running:

```bash
# Health check
curl http://localhost:8080/health

# Open Swagger UI
open http://localhost:8080/swagger-ui/

# Fetch OpenAPI spec
curl http://localhost:8080/api-doc/openapi.json
```

## Build Performance

For day-to-day development you can skip AWS providers entirely, which removes ~8 heavy SDK crates from compilation:

```bash
# Fast local build (no AWS)
cargo build -p acteon-server

# Full build matching CI
cargo build -p acteon-server --features aws-all
```

Additional tips:

- Use `cargo nextest run` instead of `cargo test` for parallel test execution — see [Build Optimization Guide](../reference/build-optimization.md)
- Skip doctests locally with `--lib --bins --tests` (they re-link the full dependency tree per doctest)
- Profile overrides in `Cargo.toml` optimize wasmtime and ring in dev builds automatically

## What's Next?

- [Quick Start](quickstart.md) — dispatch your first action
- [Configuration](configuration.md) — configure backends, rules, and authentication
