# ClickHouse Audit Backend

The ClickHouse audit backend provides columnar storage optimized for analytics queries and fast aggregations over large volumes of audit data.

<span class="badge production">Production</span> for analytics workloads

## Configuration

```toml title="acteon.toml"
[audit]
enabled = true
backend = "clickhouse"
url = "http://localhost:8123"
prefix = "acteon_"
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~970 records/sec (with Redis state) |
| **Consistency** | Eventual |
| **Retention** | TTL-based |
| **Query** | SQL (ClickHouse dialect) |
| **Feature Flag** | `clickhouse` |

## When to Use

- High-volume audit trails (millions of records)
- Analytics dashboards and reporting
- Time-series analysis of action patterns
- When paired with Redis state backend for a fast + analytics combo

## Advantages

- **Columnar storage** — fast aggregations (count, sum, avg) over specific columns
- **Compression** — excellent compression ratios for audit data
- **Horizontal scaling** — add nodes for more capacity
- **Time-series native** — optimized for time-partitioned queries

## Setup

```bash
docker compose --profile clickhouse up -d
cargo run -p acteon-server --features clickhouse -- -c examples/clickhouse.toml
```
