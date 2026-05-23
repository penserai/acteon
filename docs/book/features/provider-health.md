# Provider Health Dashboard

The Provider Health Dashboard provides real-time visibility into the health and performance of all registered providers. Unlike circuit breakers (which operate reactively when providers fail), the health dashboard offers comprehensive observability — success rates, latency percentiles, health check status, and circuit breaker state — all accessible via a single API endpoint and in the Admin UI.

This feature is infrastructure-level and operates automatically. No configuration or rules are required — metrics collection begins as soon as providers are registered and start handling requests.

## How It Works

The gateway tracks three orthogonal dimensions of provider health:

1. **Health Check Status**: Result of the provider's `health()` method (supports readiness checks, ping tests, etc.)
2. **Circuit Breaker State**: Whether the circuit is open/closed/half-open (if circuit breakers are enabled)
3. **Execution Metrics**: Per-provider counters and latency percentiles collected during normal operation

These data sources are combined into a unified health report that updates in real-time as actions are dispatched.

### Metric Collection

The gateway collects per-provider statistics automatically during `execute_action()`:

- **Success/failure counters**: Incremented on each execution based on the provider's response
- **Latency samples**: Each request's duration is recorded in microseconds
- **Last request timestamp**: Unix milliseconds of the most recent request
- **Last error**: The most recent error message from the provider (if any)

All metrics are ephemeral (in-memory only) and reset on server restart. For historical analysis and long-term monitoring, export gateway metrics to Prometheus.

### Latency Percentiles

The gateway maintains a rolling window of the **most recent 1,000 latency samples** per provider. When you query the health dashboard, percentiles (p50, p95, p99) are computed from this buffer using a selection algorithm.

**Important**: This approach gives accurate percentiles for low-to-medium traffic providers (< 100 req/s), but for high-throughput providers (1000+ req/s), the 1,000-sample buffer represents only ~1 second of traffic. For production-grade long-term latency monitoring, use Prometheus metrics and precomputed histogram buckets.

## API Reference

### Get Provider Health

```
GET /v1/providers/health
```

Returns health status, circuit breaker state, and execution metrics for all registered providers.

**Authentication**: Requires a valid API token (all roles).

**Response (200)**:

```json
{
  "providers": [
    {
      "provider": "email",
      "healthy": true,
      "health_check_error": null,
      "circuit_breaker_state": "closed",
      "total_requests": 15482,
      "successes": 15301,
      "failures": 181,
      "success_rate": 98.83,
      "avg_latency_ms": 47.3,
      "p50_latency_ms": 32.0,
      "p95_latency_ms": 125.4,
      "p99_latency_ms": 280.0,
      "last_request_at": 1707900123456,
      "last_error": null
    },
    {
      "provider": "webhook",
      "healthy": false,
      "health_check_error": "connection refused",
      "circuit_breaker_state": "open",
      "total_requests": 230,
      "successes": 45,
      "failures": 185,
      "success_rate": 19.57,
      "avg_latency_ms": 1850.2,
      "p50_latency_ms": 1420.0,
      "p95_latency_ms": 5000.0,
      "p99_latency_ms": 10000.0,
      "last_request_at": 1707899000000,
      "last_error": "timeout after 10s"
    }
  ]
}
```

### Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `provider` | string | Provider name |
| `healthy` | bool | Whether the provider's health check passed |
| `health_check_error` | string? | Health check error message (null if healthy) |
| `circuit_breaker_state` | string? | Circuit state (`closed`, `open`, `half_open`) — null if circuit breakers are disabled |
| `total_requests` | u64 | Total requests routed to this provider since startup (0 if never used) |
| `successes` | u64 | Successful executions |
| `failures` | u64 | Failed executions |
| `success_rate` | f64 | Success rate as a percentage (0.0 to 100.0) |
| `avg_latency_ms` | f64 | Average latency in milliseconds |
| `p50_latency_ms` | f64 | Median (50th percentile) latency in milliseconds |
| `p95_latency_ms` | f64 | 95th percentile latency in milliseconds |
| `p99_latency_ms` | f64 | 99th percentile latency in milliseconds |
| `last_request_at` | i64? | Unix milliseconds of the last request (null if never executed) |
| `last_error` | string? | Most recent error message (null if none) |

## Health Status Determination

A provider is marked `healthy: true` if its health check passes. Health checks are implemented by the provider's `health()` method and can perform any validation — network connectivity, credential checks, rate limit status, etc.

**Independent of circuit breakers**: A provider can be `healthy: true` (health check passes) while `circuit_breaker_state: "open"` (too many recent failures). The health check validates *potential* readiness; the circuit breaker tracks *actual* operational health.

**Example**: An email provider's health check might verify SMTP credentials and connection, returning `healthy: true`. However, if the SMTP server then starts timing out during actual message sends, the circuit breaker will trip to `open` while `healthy` remains `true` (credentials are still valid, server is reachable, but performance is degraded).

## Memory Usage

Per-provider memory overhead is approximately **8 KB**:

- **Latency sample buffer**: 1,000 × 8 bytes (u64) = 8,000 bytes
- **Counters**: ~64 bytes (atomic u64s for total/success/failure/latency)
- **Last error string**: ~100 bytes average (variable)

For a typical deployment with 5-10 providers, total memory overhead is ~50-80 KB. Even with 100 providers, the total footprint is under 1 MB.

## Thread Safety

All metrics use lock-free atomic operations where possible (counters, timestamps) and a short-duration `parking_lot::Mutex` for the latency sample buffer and last-error string. The latency buffer lock is held only for the time required to:

1. Push a new sample (O(1) with `VecDeque::push_back`)
2. Evict the oldest sample if the buffer is full (O(1) with `VecDeque::pop_front`)

Percentile computation (during `snapshot()`) acquires the lock once to copy the buffer, then releases it and computes percentiles on the copy. This ensures dashboard queries do not block live action dispatch.

## Integration with Circuit Breaker

The health dashboard displays the current circuit breaker state for each provider. When circuit breakers are enabled, you'll see:

- `circuit_breaker_state: "closed"` — Normal operation
- `circuit_breaker_state: "open"` — Circuit is open (requests rejected or rerouted)
- `circuit_breaker_state: "half_open"` — Circuit is probing for recovery

If circuit breakers are disabled, `circuit_breaker_state` is `null`.

The circuit breaker state is read from the distributed state store on each health dashboard request, ensuring multi-instance deployments show consistent data.

## Admin UI

The Admin UI includes a dedicated **Provider Health** page accessible from the main navigation. The dashboard displays:

- **Provider list** with status indicators (green = healthy, red = unhealthy)
- **Success rate** as a percentage with visual bar chart
- **Latency percentiles** (p50/p95/p99) in milliseconds
- **Circuit breaker badge** showing current state (Closed/Open/Half-Open)
- **Last error message** (if any)
- **Last request timestamp** in human-readable format
- **Auto-refresh** every 5 seconds (configurable)

The UI uses the same `GET /v1/providers/health` API endpoint consumed by external dashboards.

## Configuration

No special configuration is required. The provider health dashboard works automatically when:

1. Providers are registered via `GatewayBuilder::provider()`
2. The server is running

Health checks run on-demand when the dashboard is queried (not on a background schedule). This ensures the health status is always fresh without adding background load.

Circuit breaker state is only included if circuit breakers are enabled via the `[circuit_breaker]` config section. See the [Circuit Breaker](circuit-breaker.md) documentation for details.

## Use Cases

### Incident Response

When investigating an outage or degraded performance, the health dashboard provides immediate visibility into which providers are failing and why:

```bash
curl -H "Authorization: Bearer <token>" \
  http://localhost:8080/v1/providers/health | jq '.providers[] | select(.success_rate < 90)'
```

This query returns all providers with < 90% success rate, showing you where to focus remediation efforts.

### Capacity Planning

Use latency percentiles to identify providers approaching saturation. If p95 or p99 latencies start climbing, it may indicate the provider is reaching capacity limits or experiencing network congestion.

### Circuit Breaker Tuning

The health dashboard helps calibrate circuit breaker thresholds. If a provider's `success_rate` is 85% but its circuit is `open`, you may want to increase the `failure_threshold`. Conversely, if a provider's `success_rate` is 50% but the circuit is `closed`, the threshold may be too lenient.

### SLA Monitoring

Integrate the health dashboard API into your monitoring stack (Grafana, DataDog, etc.) to track provider SLAs over time. Set alerts when success rates drop below acceptable thresholds or when p99 latencies exceed SLA targets.

## Example: Rust Client

```rust
use acteon_client::ActeonClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ActeonClient::new("http://localhost:8080", "your-api-token")?;

    let health = client.list_provider_health().await?;

    for provider in &health.providers {
        println!("{}: {} ({}% success, p99: {:.1}ms)",
            provider.provider,
            if provider.healthy { "✓" } else { "✗" },
            provider.success_rate,
            provider.p99_latency_ms
        );

        if let Some(state) = &provider.circuit_breaker_state {
            println!("  Circuit: {}", state);
        }

        if let Some(err) = &provider.last_error {
            println!("  Last error: {}", err);
        }
    }

    Ok(())
}
```

## Limitations

### In-Memory Only

All metrics are ephemeral and reset on server restart. This is by design — the health dashboard is intended for real-time operational visibility, not long-term trend analysis.

For historical metrics and dashboards, use the gateway's Prometheus `/metrics` endpoint, which exports the same counters in a format compatible with long-term storage and alerting (Prometheus, Grafana, Thanos, etc.).

### High-Throughput Latency Accuracy

The 1,000-sample latency buffer provides accurate percentiles for providers handling up to ~100 req/s. Beyond that, the buffer represents only the most recent ~1-10 seconds of traffic, which may not reflect long-term performance.

For high-throughput providers (1000+ req/s), use Prometheus histogram metrics with precomputed buckets instead of the in-memory percentile buffer.

### No Historical Trend Data

The dashboard shows current snapshot data only. It cannot answer questions like "What was the p99 latency 3 hours ago?" or "How has success rate changed over the last week?"

For time-series queries, export metrics to Prometheus and query using PromQL.

## Design Notes

- **Why in-memory instead of state store?** Metrics are ephemeral by nature and don't need durability. Storing them in the state backend would add latency and storage overhead with no operational benefit.

- **Why 1,000 samples?** This strikes a balance between accuracy (sufficient for stable p99 estimates) and memory overhead (~8 KB per provider). Increasing to 10,000 samples would improve accuracy for high-throughput providers but increase memory usage 10x.

- **Why compute percentiles on-query instead of pre-aggregating?** Pre-aggregation (e.g., maintaining sorted buckets) would reduce query latency but increase complexity and lock contention during high-throughput dispatch. The current approach optimizes for dispatch performance (no locks during latency recording) at the expense of slightly slower dashboard queries (percentile computation takes ~1-2ms for 1,000 samples).

- **Why include health check status?** Some failures are environmental (network issues, credential expiry) rather than operational (high latency, rate limits). The health check status disambiguates these failure modes.
