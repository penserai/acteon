use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The type of analytics metric to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsMetric {
    /// Total action volume over time.
    Volume,
    /// Breakdown of outcomes (executed, failed, suppressed, etc.) over time.
    OutcomeBreakdown,
    /// Top action types by frequency.
    TopActionTypes,
    /// Latency percentiles (p50, p95, p99) over time.
    Latency,
    /// Error rate (fraction of failed actions) over time.
    ErrorRate,
}

/// Time interval for bucketing analytics data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsInterval {
    /// One-hour buckets.
    Hourly,
    /// One-day buckets.
    Daily,
    /// One-week buckets.
    Weekly,
    /// One-month buckets.
    Monthly,
}

/// Query parameters for the analytics API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnalyticsQuery {
    /// The metric to compute.
    pub metric: AnalyticsMetric,
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Filter by provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Filter by action type.
    #[serde(default)]
    pub action_type: Option<String>,
    /// Filter by outcome.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Time bucket interval (default: daily).
    #[serde(default = "default_interval")]
    pub interval: AnalyticsInterval,
    /// Start of the time range (inclusive). Defaults to 7 days ago.
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    /// End of the time range (inclusive). Defaults to now.
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    /// Dimension to group by (e.g. "provider", "`action_type`", "outcome").
    #[serde(default)]
    pub group_by: Option<String>,
    /// Number of top entries to return for `TopActionTypes` (default: 10).
    #[serde(default)]
    pub top_n: Option<usize>,
}

fn default_interval() -> AnalyticsInterval {
    AnalyticsInterval::Daily
}

/// A single time bucket in an analytics response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnalyticsBucket {
    /// Start of the time bucket.
    pub timestamp: DateTime<Utc>,
    /// Number of actions in this bucket.
    pub count: u64,
    /// Group label when `group_by` is set (e.g. the provider name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Average duration in milliseconds (latency metric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_duration_ms: Option<f64>,
    /// 50th percentile duration in milliseconds (latency metric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_duration_ms: Option<f64>,
    /// 95th percentile duration in milliseconds (latency metric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_duration_ms: Option<f64>,
    /// 99th percentile duration in milliseconds (latency metric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p99_duration_ms: Option<f64>,
    /// Error rate as a fraction (0.0 to 1.0) in this bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_rate: Option<f64>,
}

/// An entry in the top-N ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnalyticsTopEntry {
    /// The label (e.g. action type name).
    pub label: String,
    /// Total count of actions.
    pub count: u64,
    /// Percentage of total (0.0 to 100.0).
    pub percentage: f64,
}

/// Response from the analytics API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnalyticsResponse {
    /// The metric that was computed.
    pub metric: AnalyticsMetric,
    /// The interval used for bucketing.
    pub interval: AnalyticsInterval,
    /// Start of the query time range.
    pub from: DateTime<Utc>,
    /// End of the query time range.
    pub to: DateTime<Utc>,
    /// Time-bucketed data points.
    pub buckets: Vec<AnalyticsBucket>,
    /// Top-N entries (populated for `TopActionTypes` metric).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_entries: Vec<AnalyticsTopEntry>,
    /// Total count of actions in the query range.
    pub total_count: u64,
}
