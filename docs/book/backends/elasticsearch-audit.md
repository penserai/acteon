# Elasticsearch Audit Backend

The Elasticsearch audit backend provides full-text search capabilities across audit records with flexible query syntax and index lifecycle management.

<span class="badge production">Production</span> for search-heavy workloads

## Configuration

```toml title="acteon.toml"
[audit]
enabled = true
backend = "elasticsearch"
url = "http://localhost:9200"
prefix = "acteon-"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | string | — | Elasticsearch endpoint |
| `prefix` | string | `"acteon-"` | Index name prefix |

!!! note "No TTL"
    The Elasticsearch backend does not use `ttl_seconds`. Instead, configure retention via Elasticsearch's built-in [Index Lifecycle Management (ILM)](https://www.elastic.co/guide/en/elasticsearch/reference/current/index-lifecycle-management.html).

## Characteristics

| Property | Value |
|----------|-------|
| **Consistency** | Eventually consistent |
| **Search** | Full-text with relevance scoring |
| **Retention** | Index Lifecycle Management |
| **Feature Flag** | `elasticsearch` |

## When to Use

- Full-text search across audit payloads
- Complex query patterns (fuzzy matching, wildcards, aggregations)
- Existing Elasticsearch/Kibana infrastructure
- When you need to search inside action payloads

## Advantages

- **Full-text search** — search across all fields including nested payload data
- **Kibana integration** — visualize audit data with dashboards
- **Flexible queries** — boolean, range, wildcard, fuzzy, and aggregation queries
- **Auto-scaling** — add shards and replicas for capacity

## Setup

```bash
docker compose --profile elasticsearch up -d
cargo run -p acteon-server --features elasticsearch -- -c examples/elasticsearch-audit.toml
```

## Example: Combined with Redis State

A common production setup:

```toml title="examples/elasticsearch-audit.toml"
[state]
backend = "redis"
url = "redis://localhost:6379"

[audit]
enabled = true
backend = "elasticsearch"
url = "http://localhost:9200"
prefix = "acteon-"
store_payload = true
```

```bash
docker compose --profile elasticsearch up -d
cargo run -p acteon-server --features elasticsearch -- -c examples/elasticsearch-audit.toml
```
