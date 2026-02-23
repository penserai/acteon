use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use acteon_audit::analytics::AnalyticsStore;
use acteon_audit::error::AuditError;
use acteon_core::analytics::{
    AnalyticsBucket, AnalyticsInterval, AnalyticsMetric, AnalyticsQuery, AnalyticsResponse,
    AnalyticsTopEntry,
};

/// Map an `AnalyticsInterval` to the Postgres `date_trunc` argument.
fn interval_to_trunc(interval: AnalyticsInterval) -> &'static str {
    match interval {
        AnalyticsInterval::Hourly => "hour",
        AnalyticsInterval::Daily => "day",
        AnalyticsInterval::Weekly => "week",
        AnalyticsInterval::Monthly => "month",
    }
}

/// Build a WHERE clause with positional parameters from the analytics query.
///
/// Returns `(clause, binds, next_bind_idx)` where `clause` is either empty
/// or starts with `WHERE `.  Time range binds are handled separately because
/// they use `DateTime<Utc>` rather than `String`.
fn build_analytics_where(
    query: &AnalyticsQuery,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> (String, Vec<String>, u32) {
    let mut conditions = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    let mut idx = 1u32;

    let string_filters: &[(&Option<String>, &str)] = &[
        (&query.namespace, "namespace"),
        (&query.tenant, "tenant"),
        (&query.provider, "provider"),
        (&query.action_type, "action_type"),
        (&query.outcome, "outcome"),
    ];

    for (value, col) in string_filters {
        if let Some(v) = value {
            conditions.push(format!("{col} = ${idx}"));
            binds.push(v.clone());
            idx += 1;
        }
    }

    // Time range is always applied.
    conditions.push(format!("dispatched_at >= ${idx}"));
    let from_idx = idx;
    idx += 1;
    conditions.push(format!("dispatched_at <= ${idx}"));
    let to_idx = idx;
    idx += 1;

    let _ = (from_idx, to_idx, from, to); // used by caller for binding

    let clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    (clause, binds, idx)
}

/// Lightweight analytics store backed by a Postgres connection pool.
///
/// Created via the `analytics()` method on `PostgresAuditStore` — shares the same pool.
pub struct PostgresAnalyticsStore {
    pool: PgPool,
    table: String,
}

impl PostgresAnalyticsStore {
    /// Create a new `PostgresAnalyticsStore`.
    pub fn new(pool: PgPool, table: String) -> Self {
        Self { pool, table }
    }

    fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn table_name(&self) -> &str {
        &self.table
    }
}

/// Row type for volume/outcome/error-rate bucket queries.
#[derive(sqlx::FromRow)]
struct BucketRow {
    bucket: DateTime<Utc>,
    cnt: i64,
    #[sqlx(default)]
    group_label: Option<String>,
    #[sqlx(default)]
    avg_dur: Option<f64>,
    #[sqlx(default)]
    p50_dur: Option<f64>,
    #[sqlx(default)]
    p95_dur: Option<f64>,
    #[sqlx(default)]
    p99_dur: Option<f64>,
    #[sqlx(default)]
    failed_cnt: Option<i64>,
}

/// Row type for top-N queries.
#[derive(sqlx::FromRow)]
struct TopRow {
    label: String,
    cnt: i64,
}

#[async_trait]
impl AnalyticsStore for PostgresAnalyticsStore {
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
        let trunc = interval_to_trunc(query.interval);
        let top_n = query.top_n.unwrap_or(10);

        let (where_clause, binds, next_idx) = build_analytics_where(query, from, to);

        // Determine the group-by SQL expression.
        let group_col = query.group_by.as_deref().and_then(|dim| match dim {
            "provider" | "action_type" | "outcome" | "namespace" | "tenant" => Some(dim),
            _ => None,
        });

        let (group_select, group_by_clause) = if let Some(col) = group_col {
            (format!(", {col} AS group_label"), format!(", {col}"))
        } else {
            (", NULL::text AS group_label".to_string(), String::new())
        };

        // Build the main aggregation query based on metric.
        let extra_selects = match query.metric {
            AnalyticsMetric::Latency => ", AVG(duration_ms) AS avg_dur, \
                 PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY duration_ms) AS p50_dur, \
                 PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY duration_ms) AS p95_dur, \
                 PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY duration_ms) AS p99_dur, \
                 NULL::bigint AS failed_cnt"
                .to_string(),
            AnalyticsMetric::ErrorRate => ", NULL::float8 AS avg_dur, NULL::float8 AS p50_dur, \
                 NULL::float8 AS p95_dur, NULL::float8 AS p99_dur, \
                 COUNT(*) FILTER (WHERE outcome = 'failed') AS failed_cnt"
                .to_string(),
            _ => ", NULL::float8 AS avg_dur, NULL::float8 AS p50_dur, \
                 NULL::float8 AS p95_dur, NULL::float8 AS p99_dur, \
                 NULL::bigint AS failed_cnt"
                .to_string(),
        };

        let sql = format!(
            "SELECT date_trunc('{trunc}', dispatched_at) AS bucket, \
             COUNT(*) AS cnt{group_select}{extra_selects} \
             FROM {table} {where_clause} \
             GROUP BY bucket{group_by_clause} \
             ORDER BY bucket ASC",
            table = self.table_name(),
        );

        let mut q = sqlx::query_as::<_, BucketRow>(&sql);
        for b in &binds {
            q = q.bind(b);
        }
        q = q.bind(from).bind(to);

        let rows: Vec<BucketRow> = q
            .fetch_all(self.pool())
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        // Convert rows to AnalyticsBuckets.
        let mut total_count = 0u64;
        let buckets: Vec<AnalyticsBucket> = rows
            .into_iter()
            .map(|row| {
                #[allow(clippy::cast_sign_loss)]
                let count = row.cnt as u64;
                total_count += count;

                let error_rate = if query.metric == AnalyticsMetric::ErrorRate {
                    let failed = row.failed_cnt.unwrap_or(0);
                    if row.cnt > 0 {
                        Some(failed as f64 / row.cnt as f64)
                    } else {
                        Some(0.0)
                    }
                } else {
                    None
                };

                AnalyticsBucket {
                    timestamp: row.bucket,
                    count,
                    group: row.group_label,
                    avg_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        row.avg_dur
                    } else {
                        None
                    },
                    p50_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        row.p50_dur
                    } else {
                        None
                    },
                    p95_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        row.p95_dur
                    } else {
                        None
                    },
                    p99_duration_ms: if query.metric == AnalyticsMetric::Latency {
                        row.p99_dur
                    } else {
                        None
                    },
                    error_rate,
                }
            })
            .collect();

        // Top-N query for TopActionTypes metric.
        let top_entries = if query.metric == AnalyticsMetric::TopActionTypes {
            let top_sql = format!(
                "SELECT action_type AS label, COUNT(*) AS cnt \
                 FROM {table} {where_clause} \
                 GROUP BY action_type \
                 ORDER BY cnt DESC \
                 LIMIT {top_n}",
                table = self.table_name(),
            );

            let mut tq = sqlx::query_as::<_, TopRow>(&top_sql);
            for b in &binds {
                tq = tq.bind(b);
            }
            tq = tq.bind(from).bind(to);

            let top_rows: Vec<TopRow> = tq
                .fetch_all(self.pool())
                .await
                .map_err(|e| AuditError::Storage(e.to_string()))?;

            top_rows
                .into_iter()
                .map(|row| {
                    #[allow(clippy::cast_sign_loss)]
                    let count = row.cnt as u64;
                    let pct = if total_count > 0 {
                        (count as f64 / total_count as f64) * 100.0
                    } else {
                        0.0
                    };
                    AnalyticsTopEntry {
                        label: row.label,
                        count,
                        percentage: pct,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Suppress unused variable warning.
        let _ = next_idx;

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
