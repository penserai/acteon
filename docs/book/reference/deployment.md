# Deployment

## Database Migrations

Before starting the server for the first time (or after upgrading), run migrations to initialize database schemas:

```bash
# Using the wrapper script (auto-detects backend from config)
scripts/migrate.sh -c acteon.toml

# Or directly via the server binary
cargo run -p acteon-server --features postgres -- -c acteon.toml migrate

# In Docker
docker run --rm \
  -v $(pwd)/acteon.toml:/app/acteon.toml \
  acteon -c /app/acteon.toml migrate
```

Migrations are idempotent — they use `IF NOT EXISTS` patterns and are safe to run on every deploy. Backends that don't require schemas (memory, Redis) are no-ops.

## Docker

### Building

```bash
docker build -t acteon .
```

### Running

```bash
# With in-memory backend
docker run -p 8080:8080 acteon

# With config file (run migrations first, then start)
docker run --rm \
  -v $(pwd)/acteon.toml:/app/acteon.toml \
  acteon -c /app/acteon.toml migrate

docker run -p 8080:8080 \
  -v $(pwd)/acteon.toml:/app/acteon.toml \
  -v $(pwd)/rules:/app/rules \
  acteon -c /app/acteon.toml
```

### Docker Compose

```yaml title="docker-compose.yml"
services:
  acteon:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - ./acteon.toml:/app/acteon.toml
      - ./rules:/app/rules
    command: ["-c", "/app/acteon.toml"]
    depends_on:
      - redis
      - postgres

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"

  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: acteon
      POSTGRES_PASSWORD: acteon
      POSTGRES_DB: acteon
    ports:
      - "5432:5432"
```

## Production Configuration

### Recommended Setup

```toml title="acteon.toml"
[server]
host = "0.0.0.0"
port = 8080
shutdown_timeout_seconds = 30

[state]
backend = "redis"
url = "redis://redis:6379"
prefix = "acteon"

[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@postgres:5432/acteon"
prefix = "acteon_"
ttl_seconds = 2592000
cleanup_interval_seconds = 3600
store_payload = true

[audit.redact]
enabled = true
fields = ["password", "token", "api_key", "secret"]
placeholder = "[REDACTED]"

[rules]
directory = "/app/rules"

[executor]
max_retries = 3
timeout_seconds = 30
max_concurrent = 100

[auth]
enabled = true
config_path = "/app/auth.toml"
watch = true
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log verbosity: `error`, `warn`, `info`, `debug`, `trace` |
| `OPENAI_API_KEY` | LLM guardrail API key |

### Logging

```bash
# Production (errors and warnings only)
RUST_LOG=warn cargo run -p acteon-server

# Debug
RUST_LOG=debug cargo run -p acteon-server

# Per-crate filtering
RUST_LOG=acteon_gateway=debug,acteon_server=info cargo run -p acteon-server
```

## Health Monitoring

### Health Check

```bash
# Kubernetes liveness probe
curl -f http://localhost:8080/health

# Readiness check
curl -sf http://localhost:8080/health | jq -e '.status == "ok"'
```

### Metrics

```bash
curl http://localhost:8080/metrics
```

Returns dispatch counters for monitoring dashboards:

```json
{
  "dispatched": 15000,
  "executed": 12000,
  "deduplicated": 1500,
  "suppressed": 500,
  "rerouted": 300,
  "throttled": 200,
  "failed": 100
}
```

## Graceful Shutdown

Acteon supports graceful shutdown. When receiving SIGTERM:

1. Stop accepting new connections
2. Wait for in-flight requests to complete (up to `shutdown_timeout_seconds`)
3. Flush pending event groups
4. Close database connections
5. Exit

```toml
[server]
shutdown_timeout_seconds = 30
```

## High Availability

### Multi-Instance Deployment

Run multiple Acteon instances behind a load balancer:

```
                    ┌─── Acteon Instance 1 ───┐
Load Balancer ──────┼─── Acteon Instance 2 ───┼──── Redis / PostgreSQL
                    └─── Acteon Instance 3 ───┘
```

Requirements:

- **Shared state backend** (Redis, PostgreSQL, or DynamoDB)
- **Shared audit backend** (PostgreSQL, ClickHouse, or Elasticsearch)
- **Shared rules directory** (mounted volume or config management)

### Backend Recommendations

| Scenario | State | Audit |
|----------|-------|-------|
| General production | Redis | PostgreSQL |
| Strict consistency | PostgreSQL | PostgreSQL |
| AWS-native | DynamoDB | PostgreSQL |
| Analytics-heavy | Redis | ClickHouse |

## Security Checklist

- [ ] Enable authentication (`[auth].enabled = true`)
- [ ] Enable audit redaction (`[audit.redact].enabled = true`)
- [ ] Use TLS (via reverse proxy like nginx)
- [ ] Restrict network access to backends
- [ ] Rotate API keys and JWT secrets regularly
- [ ] Set appropriate `ttl_seconds` for audit retention
- [ ] Monitor `/metrics` for anomalous patterns
- [ ] Configure `max_concurrent` to prevent resource exhaustion
