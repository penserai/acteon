# Backends

Acteon uses pluggable backends for both **state storage** and **audit trail** persistence. State and audit backends are independent — you can mix any state backend with any audit backend.

## Backend Categories

### State Backends

State backends store distributed locks, deduplication keys, event state, group state, and chain state.

| Backend | Consistency | Throughput | Use Case |
|---------|------------|------------|----------|
| [Memory](memory.md) | Perfect | ~50,000/s | Development, testing |
| [Redis](redis.md) | Strong | ~2,000/s | General purpose, most deployments |
| [PostgreSQL](postgres.md) | ACID | ~850/s | Strong consistency requirements |
| [DynamoDB](dynamodb.md) | Strong | ~340/s | AWS-native deployments |
| [ClickHouse](clickhouse-state.md) | Eventual | ~120/s | Analytics (not for dedup) |

### Audit Backends

Audit backends store the searchable history of every action and its outcome.

| Backend | Best For | Features |
|---------|----------|----------|
| Memory | Testing | No persistence |
| [PostgreSQL](postgres-audit.md) | Production | ACID, indexed queries, TTL |
| [ClickHouse](clickhouse-audit.md) | Analytics | Columnar, fast aggregations |
| [Elasticsearch](elasticsearch-audit.md) | Search | Full-text search, ILM |

## Recommended Combinations

| Use Case | State | Audit | Why |
|----------|-------|-------|-----|
| **Development** | Memory | Memory | Zero dependencies |
| **Production (general)** | Redis | PostgreSQL | Fast state + reliable audit |
| **Production (strict)** | PostgreSQL | PostgreSQL | ACID everywhere |
| **Analytics-heavy** | Redis | ClickHouse | Fast state + analytics |
| **Search-heavy** | Redis | Elasticsearch | Fast state + full-text search |
| **AWS-native** | DynamoDB | PostgreSQL | Managed services |

## Mixing Backends

```toml title="acteon.toml"
# Redis for state (fast distributed locking)
[state]
backend = "redis"
url = "redis://localhost:6379"

# PostgreSQL for audit (reliable, queryable)
[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"
```

```bash
# Start both backends
docker compose --profile postgres up -d
scripts/migrate.sh -c acteon.toml
cargo run -p acteon-server -- -c acteon.toml
```
