use async_trait::async_trait;
use chrono::{DateTime, Utc};

use acteon_audit::analytics::AnalyticsStore;
use acteon_audit::error::AuditError;
use acteon_core::analytics::{
    AnalyticsBucket, AnalyticsInterval, AnalyticsMetric, AnalyticsQuery, AnalyticsResponse,
    AnalyticsTopEntry,
};

/// Map an `AnalyticsInterval` to the `ClickHouse` time-truncation function.
fn interval_to_ch_func(interval: AnalyticsInterval) -> &'static str {
    match interval {
        AnalyticsInterval::Hourly => "toStartOfHour",
        AnalyticsInterval::Daily => "toStartOfDay",
        AnalyticsInterval::Weekly => "toStartOfWeek",
        AnalyticsInterval::Monthly => "toStartOfMonth",
    }
}

/// Bind value types for parameterized `ClickHouse` queries.
enum BindValue {
    Str(String),
    Millis(i64),
}

/// Build a parameterized WHERE clause from the analytics query.
///
/// Returns `(clause, binds)` where `clause` uses `?` placeholders and `binds`
/// contains the values to bind in order.
fn build_analytics_where(
    query: &AnalyticsQuery,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> (String, Vec<BindValue>) {
    let mut conditions = Vec::new();
    let mut binds = Vec::new();

    let string_filters: &[(&Option<String>, &str)] = &[
        (&query.namespace, "namespace"),
        (&query.tenant, "tenant"),
        (&query.provider, "provider"),
        (&query.action_type, "action_type"),
        (&query.outcome, "outcome"),
    ];

    for (value, col) in string_filters {
        if let Some(v) = value {
            conditions.push(format!("{col} = ?"));
            binds.push(BindValue::Str(v.clone()));
        }
    }

    // Time range: dispatched_at is stored as milliseconds since epoch.
    conditions.push("dispatched_at >= ?".to_string());
    binds.push(BindValue::Millis(from.timestamp_millis()));
    conditions.push("dispatched_at <= ?".to_string());
    binds.push(BindValue::Millis(to.timestamp_millis()));

    let clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    (clause, binds)
}

/// Apply bind values to a `ClickHouse` query in order.
fn apply_binds(mut q: clickhouse::query::Query, binds: &[BindValue]) -> clickhouse::query::Query {
    for b in binds {
        match b {
            BindValue::Str(s) => q = q.bind(s.as_str()),
            BindValue::Millis(ms) => q = q.bind(*ms),
        }
    }
    q
}

/// Lightweight analytics store backed by a `ClickHouse` client.
///
/// Created via the `analytics()` method on `ClickHouseAuditStore` — shares the same client.
pub struct ClickHouseAnalyticsStore {
    client: clickhouse::Client,
    table: String,
}

impl ClickHouseAnalyticsStore {
    /// Create a new `ClickHouseAnalyticsStore`.
    pub fn new(client: clickhouse::Client, table: String) -> Self {
        Self { client, table }
    }

    fn client(&self) -> &clickhouse::Client {
        &self.client
    }

    fn table_name(&self) -> &str {
        &self.table
    }
}

/// Row type for bucket aggregation results.
#[derive(clickhouse::Row, serde::Deserialize)]
struct BucketRow {
    /// Bucket timestamp as milliseconds since epoch.
    bucket: i64,
    cnt: u64,
    group_label: String,
    avg_dur: f64,
    p50_dur: f64,
    p95_dur: f64,
    p99_dur: f64,
    failed_cnt: u64,
}

/// Row type for top-N queries.
#[derive(clickhouse::Row, serde::Deserialize)]
struct TopRow {
    label: String,
    cnt: u64,
}

#[async_trait]
impl AnalyticsStore for ClickHouseAnalyticsStore {
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    async fn query_analytics(
        &self,
        query: &AnalyticsQuery,
    ) -> Result<AnalyticsResponse, AuditError> {
        let now = Utc::now();
        let from = query
            .from
            .unwrap_or_else(|| now - chrono::Duration::days(7));
        let to = query.to.unwrap_or(now);
        let trunc_fn = interval_to_ch_func(query.interval);
        let top_n = query.top_n.unwrap_or(10);

        let (where_clause, binds) = build_analytics_where(query, from, to);

        // ClickHouse stores dispatched_at as Int64 (millis).
        // Convert to DateTime64 for truncation.
        let bucket_expr =
            format!("toUnixTimestamp64Milli({trunc_fn}(fromUnixTimestamp64Milli(dispatched_at)))");

        let group_col = query.group_by.as_deref().and_then(|dim| match dim {
            "provider" | "action_type" | "outcome" | "namespace" | "tenant" => Some(dim),
            _ => None,
        });

        let (group_select, group_by_extra) = if let Some(col) = group_col {
            (format!(", {col} AS group_label"), format!(", {col}"))
        } else {
            (", '' AS group_label".to_string(), String::new())
        };

        let extra_selects = match query.metric {
            AnalyticsMetric::Latency => ", avg(duration_ms) AS avg_dur, \
                 quantile(0.5)(duration_ms) AS p50_dur, \
                 quantile(0.95)(duration_ms) AS p95_dur, \
                 quantile(0.99)(duration_ms) AS p99_dur, \
                 0 AS failed_cnt"
                .to_string(),
            AnalyticsMetric::ErrorRate => {
                ", 0 AS avg_dur, 0 AS p50_dur, 0 AS p95_dur, 0 AS p99_dur, \
                 countIf(outcome = 'failed') AS failed_cnt"
                    .to_string()
            }
            _ => ", 0 AS avg_dur, 0 AS p50_dur, 0 AS p95_dur, 0 AS p99_dur, 0 AS failed_cnt"
                .to_string(),
        };

        let sql = format!(
            "SELECT {bucket_expr} AS bucket, \
             count() AS cnt{group_select}{extra_selects} \
             FROM {table} {where_clause} \
             GROUP BY bucket{group_by_extra} \
             ORDER BY bucket ASC",
            table = self.table_name(),
        );

        let q = apply_binds(self.client().query(&sql), &binds);
        let rows: Vec<BucketRow> = q
            .fetch_all::<BucketRow>()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let mut total_count = 0u64;
        let buckets: Vec<AnalyticsBucket> = rows
            .into_iter()
            .map(|row| {
                total_count += row.cnt;
                let ts = DateTime::from_timestamp_millis(row.bucket).unwrap_or_default();
                let group = if row.group_label.is_empty() {
                    None
                } else {
                    Some(row.group_label)
                };

                let error_rate = if query.metric == AnalyticsMetric::ErrorRate {
                    if row.cnt > 0 {
                        Some(row.failed_cnt as f64 / row.cnt as f64)
                    } else {
                        Some(0.0)
                    }
                } else {
                    None
                };

                AnalyticsBucket {
                    timestamp: ts,
                    count: row.cnt,
                    group,
                    avg_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        Some(row.avg_dur)
                    } else {
                        None
                    },
                    p50_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        Some(row.p50_dur)
                    } else {
                        None
                    },
                    p95_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        Some(row.p95_dur)
                    } else {
                        None
                    },
                    p99_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        Some(row.p99_dur)
                    } else {
                        None
                    },
                    error_rate,
                }
            })
            .collect();

        // Top-N query.
        let top_entries = if query.metric == AnalyticsMetric::TopActionTypes {
            let top_sql = format!(
                "SELECT action_type AS label, count() AS cnt \
                 FROM {table} {where_clause} \
                 GROUP BY action_type \
                 ORDER BY cnt DESC \
                 LIMIT {top_n}",
                table = self.table_name(),
            );

            let tq = apply_binds(self.client().query(&top_sql), &binds);
            let top_rows: Vec<TopRow> = tq
                .fetch_all::<TopRow>()
                .await
                .map_err(|e| AuditError::Storage(e.to_string()))?;

            top_rows
                .into_iter()
                .map(|row| {
                    let pct = if total_count > 0 {
                        (row.cnt as f64 / total_count as f64) * 100.0
                    } else {
                        0.0
                    };
                    AnalyticsTopEntry {
                        label: row.label,
                        count: row.cnt,
                        percentage: pct,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        Ok(AnalyticsResponse {
            metric: query.metric,
            interval: query.interval,
            from,
            to,
            buckets,
            top_entries,
            total_count,
        })
    }
}
