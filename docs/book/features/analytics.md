# Action Analytics

Acteon provides a server-side analytics API that returns time-bucketed aggregated metrics over the audit trail. Instead of fetching raw audit records and aggregating client-side, you can query pre-computed metrics directly.

## Metrics

The analytics API supports five metric types:

| Metric | Description |
|--------|-------------|
| `volume` | Total action count per time bucket |
| `outcome_breakdown` | Count per outcome (executed, failed, suppressed) -- use with `group_by=outcome` |
| `top_action_types` | Top-N action types by frequency |
| `latency` | Duration percentiles (avg, p50, p95, p99) per bucket |
| `error_rate` | Fraction of failed actions per bucket |

## API Reference

### `GET /v1/analytics`

Query parameters:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `metric` | string | Yes | -- | One of: `volume`, `outcome_breakdown`, `top_action_types`, `latency`, `error_rate` |
| `namespace` | string | No | -- | Filter by namespace |
| `tenant` | string | No | -- | Filter by tenant |
| `provider` | string | No | -- | Filter by provider |
| `action_type` | string | No | -- | Filter by action type |
| `outcome` | string | No | -- | Filter by outcome |
| `interval` | string | No | `daily` | Time bucket: `hourly`, `daily`, `weekly`, `monthly` |
| `from` | RFC 3339 | No | 7 days ago | Start of time range |
| `to` | RFC 3339 | No | now | End of time range |
| `group_by` | string | No | -- | Group dimension: `provider`, `action_type`, `outcome`, `namespace`, `tenant` |
| `top_n` | integer | No | 10 | Number of top entries for `top_action_types` |

### Response

```json
{
  "metric": "volume",
  "interval": "daily",
  "from": "2026-02-15T00:00:00Z",
  "to": "2026-02-22T00:00:00Z",
  "buckets": [
    {
      "timestamp": "2026-02-15T00:00:00Z",
      "count": 42,
      "group": null,
      "avg_duration_ms": null,
      "error_rate": null
    }
  ],
  "top_entries": [],
  "total_count": 220
}
```

## Examples

### Volume over the last 7 days

```bash
curl "http://localhost:8080/v1/analytics?metric=volume&interval=daily"
```

### Outcome breakdown by day

```bash
curl "http://localhost:8080/v1/analytics?metric=outcome_breakdown&interval=daily&group_by=outcome"
```

### Top 5 action types

```bash
curl "http://localhost:8080/v1/analytics?metric=top_action_types&top_n=5"
```

### Latency percentiles (hourly, last 24h)

```bash
curl "http://localhost:8080/v1/analytics?metric=latency&interval=hourly&from=2026-02-21T00:00:00Z"
```

### Error rate by provider

```bash
curl "http://localhost:8080/v1/analytics?metric=error_rate&interval=daily&group_by=provider"
```

### Filtered by tenant and namespace

```bash
curl "http://localhost:8080/v1/analytics?metric=volume&namespace=notifications&tenant=acme-corp&interval=hourly"
```

## Backends

The analytics engine has three backend implementations:

1. **In-Memory** (universal fallback) -- fetches raw audit records in batches and computes aggregations in memory. Works with any audit store.
2. **Postgres** -- uses native SQL aggregation with `date_trunc`, `PERCENTILE_CONT`, and `FILTER` clauses.
3. **ClickHouse** -- uses ClickHouse-native functions like `toStartOfHour`, `quantile`, and `countIf`.

The backend is chosen automatically based on your audit store configuration.

## Admin UI

The Analytics page in the Admin UI provides:

- **Metric selector** -- tabs to switch between Volume, Outcomes, Top Actions, Latency, and Error Rate views
- **Filter bar** -- namespace, tenant, provider, interval, and date range controls
- **Time-series chart** -- interactive chart showing the selected metric over time
- **Summary cards** -- total count, averages, and key statistics
- **Top-N table** -- ranked list for the Top Action Types metric

## MCP Tool

The `query_analytics` MCP tool is available for querying analytics from Claude Code:

```
metric: "volume"
namespace: "notifications"
tenant: "acme-corp"
interval: "daily"
```

## Client SDKs

All client SDKs (Rust, Python, Node.js, Go, Java) support the `query_analytics` method:

```python
# Python
response = client.query_analytics("volume", namespace="notifications", interval="hourly")
```

```typescript
// Node.js
const response = await client.queryAnalytics({ metric: 'volume', interval: 'hourly' });
```

```go
// Go
resp, err := client.QueryAnalytics(ctx, &acteon.AnalyticsQuery{Metric: "volume"})
```
