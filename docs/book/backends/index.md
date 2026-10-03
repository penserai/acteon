# Backends

Acteon separates **execution state** from **searchable action audit**. Choose each
backend for its role in your deployment, recovery, and retention requirements.

State includes deduplication keys, locks, event state, execution progress,
worker leases, durable receipts, and stream checkpoints. Some component-specific
histories and control audits live with that state. An audit backend separately
stores the configured searchable action trail. Kafka is the bus transport and
does not replace either store.

## State backends

| Backend | Fit | Operational consideration |
|---|---|---|
| [Memory](memory.md) | Local development and tests | State is lost when the process stops |
| [Redis](redis.md) | Shared coordination and execution state | Configure persistence and availability for your recovery requirements |
| [PostgreSQL](postgres.md) | Transactional database-backed state | Apply migrations and manage connection capacity |
| [DynamoDB](dynamodb.md) | AWS-managed state storage | Configure tables, access policy, and capacity |

Use the [performance guide](../reference/performance.md) for measured workloads
and benchmark assumptions. Throughput depends on the operation and deployment.

## Audit backends

When enabled, audit backends store action records and outcomes for inspection,
analytics, replay, and configured compliance controls.

| Backend | Best For | Features |
|---------|----------|----------|
| Memory | Testing | No persistence |
| [PostgreSQL](postgres-audit.md) | Production | ACID, indexed queries, TTL |
| [ClickHouse](clickhouse-audit.md) | Analytics | Columnar, fast aggregations |
| [Elasticsearch](elasticsearch-audit.md) | Search | Full-text search, ILM |
| [DynamoDB](dynamodb-audit.md) | AWS-native / Compliance | Fully managed, GSIs, native TTL, hash chain CAS |

## Recommended Combinations

| Use Case | State | Audit | Why |
|----------|-------|-------|-----|
| **Development** | Memory | Memory | Zero dependencies |
| **Production (general)** | Redis | PostgreSQL | Fast state + reliable audit |
| **PostgreSQL operations** | PostgreSQL | PostgreSQL | Operate state and audit on a common database engine |
| **Analytics-heavy** | Redis | ClickHouse | Fast state + analytics |
| **Search-heavy** | Redis | Elasticsearch | Fast state + full-text search |
| **AWS-native** | DynamoDB | DynamoDB or PostgreSQL | Fully managed AWS infrastructure |

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
cargo run -p acteon-server --features postgres -- -c acteon.toml
```
