# Grafana Dashboard Templates -- Architecture

## Overview

The monitoring stack consists of three components:

1. **Prometheus exporter** -- An Axum handler at `GET /metrics/prometheus` that serializes in-memory atomic counters to Prometheus text exposition format.
2. **Prometheus scraper** -- A standard Prometheus instance configured to scrape the Acteon endpoint on a 15-second interval.
3. **Grafana dashboards** -- Two pre-built JSON dashboard definitions provisioned automatically via Grafana's file-based provisioning.

## Metrics Pipeline

```
AtomicU64 / AtomicI64 counters
  (GatewayMetrics, ProviderMetrics, EmbeddingMetrics)
        │
        ▼
    snapshot()
  (copies atomic values into plain structs:
   MetricsSnapshot, ProviderStatsSnapshot, EmbeddingStatsSnapshot)
        │
        ▼
  prometheus_metrics() handler
  (formats snapshots into Prometheus text exposition format)
        │
        ▼
  HTTP response: text/plain; version=0.0.4
        │
        ▼
  Prometheus scraper (15s interval)
        │
        ▼
  Grafana queries via PromQL
```

### Counter Storage

All counters live in `GatewayMetrics` (`crates/gateway/src/metrics.rs`), a struct of `AtomicU64` fields using `Ordering::Relaxed`. This means:

- **No locks during dispatch** -- incrementing a counter is a single atomic fetch-add, with no contention between dispatch threads.
- **Relaxed ordering** -- individual counter reads may be slightly stale relative to each other. The `snapshot()` method reads all counters in sequence, providing a near-consistent point-in-time view. For monitoring purposes (15-second scrape intervals), this is sufficient.

Per-provider metrics use a separate `ProviderMetrics` struct (`crates/gateway/src/provider_metrics.rs`) that maintains per-provider counters and a rolling latency sample buffer. The `snapshot()` method computes percentiles (p50, p95, p99) from the buffer at read time.

Embedding metrics follow the same pattern with `EmbeddingMetrics`.

### Serialization

The `prometheus_metrics()` handler in `crates/server/src/api/prometheus.rs` serializes counters using four helper functions:

- `write_counter()` -- emits `# HELP`, `# TYPE counter`, and the value line for a single scalar counter.
- `write_provider_counter_header()` / `write_provider_gauge_header()` -- emit `# HELP` and `# TYPE` lines for labeled metrics (one header, multiple value lines).
- `write_labeled_value()` / `write_labeled_float()` -- emit a single metric line with a `provider="..."` label.

The output follows the [Prometheus text exposition format v0.0.4](https://prometheus.io/docs/instrumenting/exposition_formats/#text-based-format) specification. The content type is set to `text/plain; version=0.0.4; charset=utf-8`.

## Design Decisions

### Zero-Dependency Exporter

The Prometheus exporter is implemented as a hand-written text formatter (~80 lines) rather than using the `prometheus` or `opentelemetry-prometheus` crates. Rationale:

- **No additional dependencies** -- Acteon's dependency tree stays lean. The `prometheus` crate pulls in `protobuf` and several supporting crates.
- **Full control over metric naming** -- All metrics use the `acteon_` prefix consistently without framework-imposed naming conventions.
- **Simple mapping** -- The gateway already maintains `AtomicU64` counters. The exporter is a direct read-and-format pass with no intermediate registry or collector abstraction.
- **Text format over protobuf** -- Prometheus text format is human-readable, easy to debug with `curl`, and universally supported by all Prometheus-compatible scrapers. The protobuf exposition format offers marginal size savings but adds complexity and a protobuf dependency.

### Gauge vs. Counter for Provider Metrics

Per-provider latency percentiles and success rates are emitted as **gauges** rather than counters because they represent computed point-in-time values (not monotonically increasing totals). The gateway computes these values from the rolling sample buffer during `snapshot()`, and the Prometheus exporter emits them as-is.

Per-provider request/success/failure counts are emitted as **counters** because they are monotonically increasing totals.

### Dashboard Provisioning

Dashboards are provisioned using Grafana's file-based provisioning rather than the HTTP API. This approach:

- Works without Grafana API tokens or authentication setup.
- Deploys deterministically from source control (the JSON files are the source of truth).
- Loads dashboards on Grafana startup with zero manual steps.
- Supports `editable: true`, allowing operators to customize panels in the UI while still having a known-good baseline in source control.

### Template Variable for Datasource

Both dashboards use a `DS_PROMETHEUS` template variable of type `datasource` rather than hardcoding a specific Prometheus instance. This allows the dashboards to work in environments where the Prometheus datasource has a different name or UID.

## Dashboard Structure

### Overview Dashboard (`acteon-overview.json`)

![Acteon Overview Dashboard](../images/grafana-overview.png)

Seven collapsible row sections, 17 panels total:

| Row | Panels | Metric Categories |
|-----|--------|-------------------|
| Throughput | 3 (timeseries, timeseries, stat) | `acteon_actions_*` |
| LLM Guardrail | 2 (timeseries, stat) | `acteon_llm_guardrail_*` |
| Chains | 3 (timeseries, gauge, stat) | `acteon_chains_*` |
| Circuit Breaker | 2 (timeseries, stat) | `acteon_circuit_*` |
| Recurring Actions | 2 (stat, timeseries) | `acteon_recurring_*` |
| Quotas & Retention | 2 (stat, stat) | `acteon_quota_*`, `acteon_retention_*` |
| Embedding Cache | 3 (timeseries, gauge, gauge) | `acteon_embedding_*` |

### Provider Health Dashboard (`acteon-provider-health.json`)

![Acteon Provider Health Dashboard](../images/grafana-provider-health.png)

Three collapsible row sections, 7 panels total:

| Row | Panels | Metric Categories |
|-----|--------|-------------------|
| Provider Success Rates | 1 (stat) | `acteon_provider_success_rate` |
| Request Volume | 3 (timeseries, timeseries, stat) | `acteon_provider_requests_total`, `acteon_provider_failures_total` |
| Latency | 3 (timeseries, timeseries, table) | `acteon_provider_*_latency_ms`, `acteon_provider_success_rate` |

## File Layout

```
crates/
  gateway/src/
    metrics.rs              # GatewayMetrics (AtomicU64 counters + snapshot)
    provider_metrics.rs     # Per-provider metrics (latency buffer + percentiles)
  server/src/api/
    prometheus.rs           # GET /metrics/prometheus handler

deploy/
  grafana/
    dashboards/
      acteon-overview.json
      acteon-provider-health.json
    provisioning/
      dashboards/dashboards.yml
      datasources/prometheus.yml
  prometheus/
    prometheus.yml
```

## Metric Lifecycle

1. **Dispatch time** -- Gateway increments `AtomicU64` counters in `GatewayMetrics` as actions flow through the pipeline. Provider execution records latency samples in `ProviderMetrics`.
2. **Scrape time** (every 15s) -- Prometheus sends `GET /metrics/prometheus`. The handler calls `snapshot()` on each metrics struct, reads all atomic values, computes provider percentiles, and formats the response.
3. **Query time** -- Grafana panels issue PromQL queries against Prometheus. Rate calculations (`rate(...[5m])`) are computed by Prometheus from the raw counter values.
4. **Display time** -- Grafana renders panels with 30-second auto-refresh, fetching fresh data from Prometheus on each cycle.

## Scaling Considerations

- **Multi-instance deployments** -- Each Acteon instance exposes its own `/metrics/prometheus` endpoint with instance-local counters. Prometheus scrapes all instances and Grafana queries aggregate across them. PromQL functions like `sum(rate(...))` combine per-instance counters automatically.
- **Scrape performance** -- The endpoint reads ~40 atomic counters plus per-provider stats. With 10 providers, the response is ~4 KB of text. Serialization takes < 1ms.
- **Memory overhead** -- Zero additional memory beyond the existing atomic counters. The Prometheus response string is allocated per-request (pre-allocated at 4 KB) and dropped after the response is sent.
