# Provider Health Dashboard Architecture

## Overview

The Provider Health Dashboard exposes per-provider health, performance, and circuit breaker status via `GET /v1/providers/health`. It combines three data sources into a unified health report:

1. **Health checks** — Provider-specific readiness validation (credentials, connectivity, etc.)
2. **Circuit breaker state** — Distributed state tracking operational health (open/closed/half-open)
3. **Execution metrics** — In-memory counters and latency samples collected during normal dispatch

This document describes the design decisions, data flow, thread safety approach, and trade-offs.

---

## 1. Data Model

### `ProviderHealthStatus` (Response Struct)

Defined in `crates/core/src/provider_health.rs`:

```rust
pub struct ProviderHealthStatus {
    pub provider: String,
    pub healthy: bool,
    pub health_check_error: Option<String>,
    pub circuit_breaker_state: Option<String>,
    pub total_requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub last_request_at: Option<i64>,
    pub last_error: Option<String>,
}
```

This is the unified view returned by the API. Fields are populated from multiple sources during request processing.

### `ProviderStats` (Internal Metrics Struct)

Defined in `crates/gateway/src/metrics.rs`:

```rust
pub struct ProviderStats {
    total_requests: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
    total_latency_us: AtomicU64,
    latency_samples: parking_lot::Mutex<VecDeque<u64>>,
    last_request_at: AtomicI64,
    last_error: parking_lot::Mutex<Option<String>>,
}
```

One `ProviderStats` instance per registered provider. Stored in `Gateway.provider_metrics` (a `DashMap<String, ProviderStats>`).

---

## 2. Data Flow

### API Request Processing

```
HTTP GET /v1/providers/health
   |
   v
list_provider_health (handler)
   |
   +-- gateway.check_provider_health() -> Vec<HealthCheckResult>
   |   (calls provider.health() for each registered provider)
   |
   +-- gateway.provider_metrics().snapshot() -> HashMap<String, ProviderMetricsSnapshot>
   |   (takes snapshot of all ProviderStats)
   |
   +-- gateway.circuit_breakers().get(provider).state() -> CircuitBreakerState
   |   (reads distributed state store for circuit breaker status)
   |
   v
Merge all three data sources into ProviderHealthStatus structs
   |
   v
JSON response
```

**Key points**:

- Health checks are executed **on-demand** when the dashboard is queried (not on a background schedule). This ensures fresh data without adding background load.
- Metrics are **in-memory only** (no state store reads). Snapshot takes O(num_providers) mutex locks to copy latency buffers.
- Circuit breaker state is read from the **distributed state store** (ensuring multi-instance consistency).

### Metric Collection (During Dispatch)

```
gateway.dispatch(action)
   |
   v
execute_action(provider, payload)
   |
   +-- Start timer (Instant::now())
   |
   +-- provider.execute(payload).await
   |
   +-- Stop timer, compute latency_us
   |
   +-- Update ProviderStats:
       |
       +-- On success: record_success(latency_us)
       |   |
       |   +-- total_requests.fetch_add(1)
       |   +-- successes.fetch_add(1)
       |   +-- total_latency_us.fetch_add(latency_us)
       |   +-- push_latency(latency_us) -> latency_samples buffer
       |   +-- last_request_at.store(now_ms)
       |
       +-- On failure: record_failure(latency_us, error)
           |
           +-- total_requests.fetch_add(1)
           +-- failures.fetch_add(1)
           +-- total_latency_us.fetch_add(latency_us)
           +-- push_latency(latency_us)
           +-- last_request_at.store(now_ms)
           +-- last_error.lock().replace(error)
```

**Key points**:

- Latency is measured using `std::time::Instant` (monotonic clock, not affected by system time adjustments).
- Metrics are updated **synchronously** after each provider execution (no background aggregation).
- Lock is held **only** during `push_latency()` (O(1) buffer operations).

---

## 3. Thread Safety Approach

### Atomic Counters (Lock-Free)

```rust
total_requests: AtomicU64,
successes: AtomicU64,
failures: AtomicU64,
total_latency_us: AtomicU64,
last_request_at: AtomicI64,
```

All counters use `Ordering::Relaxed` for maximum throughput. Relaxed ordering is safe because:

- Individual counter updates are atomic (no torn reads/writes).
- Counters are independent (no cross-counter invariants).
- Snapshot reads use the same relaxed ordering, accepting stale-but-consistent values.

**Trade-off**: Snapshot values may not be perfectly synchronized across counters (e.g., `total_requests` might be N+1 while `successes` is still N). This is acceptable for operational metrics.

### Latency Buffer (Short-Duration Mutex)

```rust
latency_samples: parking_lot::Mutex<VecDeque<u64>>
```

The latency buffer uses a `parking_lot::Mutex` (not `std::sync::Mutex`) because:

- **Faster uncontended lock** (parking_lot uses inline fast-path instead of syscall).
- **Smaller memory footprint** (no poisoning metadata).
- **Better performance under contention** (adaptive spinning before parking).

The mutex is held for **O(1) operations only**:

1. `push_latency()`: `VecDeque::push_back()` + optional `pop_front()` (eviction).
2. `snapshot()`: `VecDeque::clone()` (copies the entire buffer, then releases lock).

**Why not RwLock?** Write-heavy workload (every request updates the buffer). RwLock adds overhead for reader prioritization that doesn't benefit this use case.

### Last Error String (Mutex)

```rust
last_error: parking_lot::Mutex<Option<String>>
```

Updated only on failure. Most requests are successful, so lock contention is rare. `parking_lot::Mutex` minimizes overhead.

---

## 4. Percentile Calculation

### Algorithm

Percentiles are computed using the **quickselect** algorithm (average O(n), worst-case O(n²)):

```rust
fn percentile(samples: &mut [u64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let k = ((samples.len() - 1) as f64 * p) as usize;
    *samples.select_nth_unstable(k).1 as f64 / 1000.0
}
```

**Why quickselect instead of sort?**

- Sorting the entire buffer is O(n log n).
- Quickselect for a single percentile is O(n) average case.
- The `select_nth_unstable()` method is in-place (no allocation).

**Trade-off**: Computing p50, p95, and p99 separately requires three O(n) passes. A single full sort (O(n log n)) would enable all three percentiles in one pass, but we optimize for the common case (low traffic, small n) where n log n ≈ n anyway.

### Why p50/p95/p99?

- **p50 (median)**: Represents typical latency. Useful for detecting systemic slowdowns.
- **p95**: Catches outliers affecting a significant minority of requests. SLA targets often use p95.
- **p99**: Identifies tail latencies (slow queries, retries, network jitter). Critical for user-facing services.

We don't include p99.9 or higher because the 1,000-sample buffer provides insufficient resolution for sub-percentile accuracy at low traffic volumes.

---

## 5. Memory Usage Analysis

### Per-Provider Overhead

| Component | Size | Notes |
|-----------|------|-------|
| Latency buffer (`VecDeque<u64>`) | 8,000 bytes | 1,000 samples × 8 bytes/sample |
| Counters (5 × `AtomicU64`) | 40 bytes | total_requests, successes, failures, total_latency_us, last_request_at |
| Last error (`Option<String>`) | ~100 bytes | Variable (average error message length) |
| Mutex overhead (`parking_lot::Mutex` × 2) | 16 bytes | 8 bytes per mutex (pointer-sized) |
| DashMap entry overhead | ~48 bytes | Key (String) + metadata |

**Total per provider**: ~8,204 bytes (~8 KB).

### Scaling

| Providers | Total Memory |
|-----------|--------------|
| 5 | ~40 KB |
| 10 | ~80 KB |
| 50 | ~400 KB |
| 100 | ~800 KB |

Even with 100 providers, total memory overhead is under 1 MB. This is negligible compared to the gateway's base memory footprint (~10-50 MB depending on rule count and state backend).

---

## 6. Why In-Memory vs State Store?

### Decision: In-Memory Only

Metrics are **not** persisted in the state store. Rationale:

1. **Ephemeral by nature**: Operational metrics (success rate, latency) are only meaningful for the current instance. Historical trends belong in Prometheus/Grafana.

2. **No durability required**: If a gateway instance restarts, losing in-flight metrics is acceptable. The provider health dashboard rebuilds state from scratch within seconds.

3. **State store overhead**: Writing metrics to Redis/Postgres on every request would:
   - Add latency to the critical dispatch path (100-500µs per write).
   - Increase state backend load (metrics writes would dominate traffic).
   - Require TTL/cleanup logic (metrics accumulate forever).

4. **Multi-instance consistency**: Each instance has its own metrics. This is intentional — per-instance metrics help diagnose instance-specific issues (e.g., network partitions, local resource exhaustion). For aggregate metrics, use Prometheus federation.

### Comparison: State Store Approach

If metrics were persisted in the state store:

| Aspect | In-Memory | State Store |
|--------|-----------|-------------|
| Dispatch latency overhead | ~0µs (atomic increment) | ~100-500µs (state write) |
| Memory usage | ~8 KB/provider | ~0 (state backend storage) |
| Multi-instance aggregation | No (per-instance) | Yes (global view) |
| Historical data | No (restart = reset) | Yes (survives restarts) |
| State backend load | None | High (write on every request) |
| Query latency | ~1-2ms (local snapshot) | ~10-50ms (state backend query) |

**Conclusion**: In-memory is the right choice for real-time operational metrics. For historical analysis, use Prometheus.

---

## 7. High-Traffic Provider Limitations

### Problem: 1,000-Sample Buffer is Insufficient

At 1,000+ req/s, the latency buffer represents only ~1 second of traffic. This causes:

1. **Stale percentiles**: p99 reflects only the most recent second, not the last minute/hour.
2. **Loss of outlier visibility**: Rare tail latencies (e.g., 1/10,000 events) may never appear in the buffer.
3. **Churn overhead**: At 10,000 req/s, the buffer is fully overwritten 10 times per second.

### Solution: Prometheus Histograms

For high-throughput providers, use Prometheus histogram metrics instead:

```rust
use prometheus::{register_histogram_vec, HistogramVec};

lazy_static! {
    static ref PROVIDER_LATENCY: HistogramVec = register_histogram_vec!(
        "acteon_provider_latency_seconds",
        "Provider execution latency",
        &["provider"],
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]
    ).unwrap();
}

// In execute_action:
let timer = PROVIDER_LATENCY.with_label_values(&[&provider]).start_timer();
provider.execute(payload).await?;
timer.observe_duration();
```

Prometheus histograms:

- Use **precomputed buckets** (no per-request buffer allocation).
- Support **efficient percentile approximation** in PromQL (`histogram_quantile()`).
- Scale to **millions of req/s** with minimal overhead.
- Provide **long-term storage** in TSDB (Prometheus, Thanos, Cortex).

**Trade-off**: Histogram accuracy degrades for latencies outside the bucket boundaries. Choose buckets carefully based on expected latency distribution.

---

## 8. Integration with Circuit Breaker

The health dashboard reads circuit breaker state from the distributed state backend:

```rust
if let Some(registry) = gateway.circuit_breakers() {
    if let Some(cb) = registry.get(provider_name) {
        let state = cb.state().await; // Reads from state store
        circuit_breaker_state = Some(state.to_string());
    }
}
```

**Why not cache circuit state in-memory?**

- Circuit state changes are **infrequent** (trip/reset events happen at most every few seconds).
- Reading from the state store ensures **multi-instance consistency** (all instances see the same circuit state).
- Dashboard queries are **not latency-sensitive** (10-50ms query time is acceptable).

**Trade-off**: Each health dashboard query incurs O(num_providers) state store reads. For deployments with 100+ providers and high dashboard refresh rates, consider adding a 1-second TTL cache.

---

## 9. Sequence Diagram

```
User                API Handler         Gateway              ProviderStats     Circuit Breaker
 |                      |                   |                      |                 |
 |-- GET /v1/providers/health -->           |                      |                 |
 |                      |                   |                      |                 |
 |                      +-- check_provider_health() -->            |                 |
 |                      |                   |                      |                 |
 |                      |                   +-- for each provider: provider.health() |
 |                      |                   |                      |                 |
 |                      |<-- Vec<HealthCheckResult> ---------------+                 |
 |                      |                   |                      |                 |
 |                      +-- provider_metrics().snapshot() -->      |                 |
 |                      |                   |                      |                 |
 |                      |                   +-- for each provider: |                 |
 |                      |                   |   lock latency_samples                 |
 |                      |                   |   clone buffer                         |
 |                      |                   |   compute percentiles (p50/p95/p99)    |
 |                      |                   |                      |                 |
 |                      |<-- HashMap<String, ProviderMetricsSnapshot> --------------+
 |                      |                   |                      |                 |
 |                      +-- circuit_breakers().get(provider).state() ------------->  |
 |                      |                   |                      |                 |
 |                      |                   |                      +-- state_store.get(key)
 |                      |                   |                      |                 |
 |                      |<-- CircuitBreakerState --------------------------------------+
 |                      |                   |                      |                 |
 |                      +-- merge health + metrics + circuit state                   |
 |                      |                   |                      |                 |
 |<-- JSON(ProviderHealthStatus[]) --------+                      |                 |
```

---

## 10. Module / File Layout

### New Files

```
crates/core/src/provider_health.rs          -- ProviderHealthStatus, ListProviderHealthResponse
crates/gateway/src/metrics.rs               -- ProviderStats struct + percentile logic
crates/server/src/api/provider_health.rs    -- GET /v1/providers/health handler
ui/src/pages/ProviderHealth.tsx             -- Admin UI dashboard page
```

### Modified Files

```
crates/core/src/lib.rs                      -- pub mod provider_health; re-export types
crates/gateway/src/gateway.rs               -- provider_metrics DashMap, check_provider_health() method
crates/gateway/src/lib.rs                   -- pub use metrics::ProviderStats;
crates/server/src/api/mod.rs                -- Register provider_health routes
crates/server/src/api/openapi.rs            -- Register ProviderHealthStatus schema
```

---

## 11. Trade-Off Summary

| Design Choice | Alternative | Rationale |
|---------------|-------------|-----------|
| In-memory metrics | State store | Zero dispatch latency overhead, acceptable for real-time metrics |
| 1,000-sample buffer | 10,000 samples | Balance accuracy vs memory (8 KB/provider) |
| Quickselect (O(n)) | Full sort (O(n log n)) | Faster for small n (1,000 samples) |
| `parking_lot::Mutex` | `std::sync::Mutex` | 2-10x faster uncontended lock, smaller footprint |
| Relaxed atomics | `Mutex<u64>` | 10-100x faster, acceptable for eventual-consistent counters |
| On-demand health checks | Background polling | Always-fresh data, no background load |
| Per-instance metrics | Aggregated metrics | Instance-specific diagnostics, simpler implementation |

---

## 12. Performance Characteristics

### Dashboard Query Latency

| Operation | Time | Notes |
|-----------|------|-------|
| Health check (per provider) | 1-50ms | Network-dependent (provider.health() is async) |
| Metrics snapshot (per provider) | 10-50µs | Lock + buffer clone + percentile computation |
| Circuit state read (per provider) | 1-10ms | State store read (Redis/Postgres) |
| Total (10 providers) | ~20-500ms | Dominated by health checks and circuit reads |

### Dispatch Path Overhead

| Operation | Time | Notes |
|-----------|------|-------|
| `record_success()` / `record_failure()` | ~100ns | 5 atomic increments + buffer push |
| Latency buffer push (no eviction) | ~50ns | `VecDeque::push_back()` (amortized O(1)) |
| Latency buffer push (with eviction) | ~100ns | `push_back()` + `pop_front()` |
| Last-error mutex lock + update | ~200ns | Only on failure (rare) |

**Conclusion**: Metrics collection adds **~100ns per request** to the dispatch path. At 10,000 req/s, this is 0.1% overhead (negligible).

---

## 13. Future Enhancements

### Histogram-Based Percentiles

Replace the 1,000-sample buffer with HdrHistogram (High Dynamic Range Histogram):

- **Constant memory** regardless of throughput (configurable precision).
- **Accurate percentiles** at high traffic (1,000,000+ req/s).
- **Low overhead** (O(1) record, O(log percentile) query).

Trade-off: More complex implementation, 10-100 KB memory overhead per provider.

### Configurable Buffer Size

Allow operators to tune the latency sample buffer size via config:

```toml
[metrics]
latency_buffer_size = 10_000  # Default: 1_000
```

Trade-off: Increased memory usage (10x for 10,000 samples).

### Prometheus Integration

Auto-export `ProviderStats` to Prometheus metrics (`acteon_provider_requests_total`, `acteon_provider_latency_seconds`). This provides long-term storage and cross-instance aggregation.

Already partially implemented via gateway-level metrics. Extend to per-provider labels.

### Background Health Checks

Poll `provider.health()` on a background interval (default: 30s) instead of on-demand. Cache results and serve from cache on dashboard queries.

Trade-off: Stale health status (up to 30s old), background load.

### Health Check SLA Tracking

Track health check success/failure over time (separate from execution metrics). Enables "provider was unreachable 3 times in the last hour" alerts.

Requires time-series storage (Prometheus or state store with TTL keys).
