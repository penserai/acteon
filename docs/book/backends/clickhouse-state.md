# ClickHouse State Backend

The ClickHouse state backend provides eventually consistent state storage. It is optimized for analytics workloads but is **not recommended** for strict deduplication or locking.

## When to Use

- Analytics-focused deployments where eventual consistency is acceptable
- When ClickHouse is already in your infrastructure
- Scenarios where occasional duplicate processing is tolerable

!!! warning "Not Recommended for Deduplication"
    ClickHouse uses `ReplacingMergeTree` which provides eventual consistency. Under concurrent load, 10-20% of duplicate actions may be processed. Use Redis or PostgreSQL if you need strict deduplication.

## Configuration

```toml title="acteon.toml"
[state]
backend = "clickhouse"
url = "http://localhost:8123"
prefix = "acteon_"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | string | — | ClickHouse HTTP endpoint |
| `prefix` | string | `"acteon_"` | Table name prefix |

## Docker Setup

```bash
docker compose --profile clickhouse up -d
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~120 ops/sec |
| **Latency** | 100-200ms |
| **Persistence** | Full |
| **Consistency** | Eventual |
| **Mutual Exclusion** | None |
| **Feature Flag** | `clickhouse` |

## Deduplication Accuracy

| Scenario | Executed | Deduplicated |
|----------|----------|--------------|
| Sequential (20 actions) | 1 | 19 |
| Concurrent (20 actions) | 10-20 | 0-10 |

Compare with Redis or PostgreSQL which achieve perfect 1:19 ratios even under concurrent load.

## Building

```bash
cargo build -p acteon-server --features clickhouse
```
