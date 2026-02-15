use serde::{Deserialize, Serialize};

#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

/// Health and performance summary for a single provider.
///
/// ## In-Memory Metrics
///
/// All execution metrics (requests, success rate, latency percentiles) are
/// stored **in-memory** and reset to zero when the gateway restarts. These
/// metrics reflect only the time since the gateway process started.
///
/// For historical analysis and production monitoring, export metrics to
/// Prometheus, Grafana, or a similar observability backend.
///
/// ## Latency Percentiles
///
/// Percentiles (p50, p95, p99) are computed from a rolling window of the
/// **most recent 1,000 samples** per provider. For low-to-medium traffic
/// providers (< 100 req/s), this provides accurate percentile estimates.
///
/// For high-traffic providers (1000+ req/s), percentiles may only represent
/// the most recent ~1 second of traffic and may not reflect long-term
/// performance. Use Prometheus histograms for production-grade percentiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct ProviderHealthStatus {
    /// Provider name.
    #[cfg_attr(feature = "utoipa", schema(example = "email"))]
    pub provider: String,

    /// Whether the provider's health check passed.
    #[cfg_attr(feature = "utoipa", schema(example = true))]
    pub healthy: bool,

    /// Health check error message (if unhealthy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_error: Option<String>,

    /// Current circuit breaker state (`closed`, `open`, `half_open`), if configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(example = "closed"))]
    pub circuit_breaker_state: Option<String>,

    /// Total requests routed to this provider since startup.
    #[cfg_attr(feature = "utoipa", schema(example = 1500))]
    pub total_requests: u64,

    /// Successful executions.
    #[cfg_attr(feature = "utoipa", schema(example = 1480))]
    pub successes: u64,

    /// Failed executions.
    #[cfg_attr(feature = "utoipa", schema(example = 20))]
    pub failures: u64,

    /// Success rate as a percentage (0.0 to 100.0).
    #[cfg_attr(feature = "utoipa", schema(example = 98.67))]
    pub success_rate: f64,

    /// Average latency in milliseconds.
    #[cfg_attr(feature = "utoipa", schema(example = 45.2))]
    pub avg_latency_ms: f64,

    /// 50th percentile (median) latency in milliseconds.
    #[cfg_attr(feature = "utoipa", schema(example = 32.0))]
    pub p50_latency_ms: f64,

    /// 95th percentile latency in milliseconds.
    #[cfg_attr(feature = "utoipa", schema(example = 120.5))]
    pub p95_latency_ms: f64,

    /// 99th percentile latency in milliseconds.
    #[cfg_attr(feature = "utoipa", schema(example = 250.0))]
    pub p99_latency_ms: f64,

    /// Unix timestamp (milliseconds) of the last request, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(example = 1_707_900_000_000_i64))]
    pub last_request_at: Option<i64>,

    /// Most recent error message from this provider, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Response for listing provider health statuses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct ListProviderHealthResponse {
    /// Per-provider health and performance data.
    pub providers: Vec<ProviderHealthStatus>,
}
