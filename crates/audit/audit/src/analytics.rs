use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};

use acteon_core::analytics::{
    AnalyticsBucket, AnalyticsInterval, AnalyticsMetric, AnalyticsQuery, AnalyticsResponse,
    AnalyticsTopEntry,
};

use crate::error::AuditError;
use crate::record::AuditQuery;
use crate::store::AuditStore;

/// Trait for analytics query backends.
///
/// Implementations may use native SQL aggregation (Postgres, `ClickHouse`) or
/// fall back to in-memory computation over raw audit records.
#[async_trait]
pub trait AnalyticsStore: Send + Sync {
    /// Execute an analytics query and return aggregated results.
    async fn query_analytics(
        &self,
        query: &AnalyticsQuery,
    ) -> Result<AnalyticsResponse, AuditError>;
}

/// In-memory analytics implementation that works with any `AuditStore`.
///
/// Fetches raw audit records in batches and computes aggregations in memory.
/// Suitable as a universal fallback when no native SQL analytics is available.
pub struct InMemoryAnalytics<S: AuditStore + ?Sized> {
    store: std::sync::Arc<S>,
}

impl<S: AuditStore + ?Sized> InMemoryAnalytics<S> {
    /// Create a new in-memory analytics engine wrapping an audit store.
    pub fn new(store: std::sync::Arc<S>) -> Self {
        Self { store }
    }
}

/// Truncate a timestamp to the start of the given interval bucket.
fn truncate_to_interval(dt: DateTime<Utc>, interval: AnalyticsInterval) -> DateTime<Utc> {
    match interval {
        AnalyticsInterval::Hourly => Utc
            .with_ymd_and_hms(dt.year(), dt.month(), dt.day(), dt.hour(), 0, 0)
            .single()
            .unwrap_or(dt),
        AnalyticsInterval::Daily => Utc
            .with_ymd_and_hms(dt.year(), dt.month(), dt.day(), 0, 0, 0)
            .single()
            .unwrap_or(dt),
        AnalyticsInterval::Weekly => {
            let weekday = dt.weekday().num_days_from_monday();
            let start_of_week = dt.date_naive() - chrono::Duration::days(i64::from(weekday));
            Utc.from_utc_datetime(&start_of_week.and_hms_opt(0, 0, 0).unwrap_or_default())
        }
        AnalyticsInterval::Monthly => Utc
            .with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
            .single()
            .unwrap_or(dt),
    }
}

/// Compute a percentile from a sorted slice of f64 values.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = (p / 100.0) * (sorted.len() - 1) as f64;
    let lower = idx.floor() as usize;
    let upper = idx.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let frac = idx - lower as f64;
        sorted[lower] * (1.0 - frac) + sorted[upper] * frac
    }
}

/// Key for grouping buckets: (truncated timestamp, optional group label).
type BucketKey = (DateTime<Utc>, Option<String>);

/// Accumulated data for a single bucket during aggregation.
struct BucketAccum {
    count: u64,
    failed_count: u64,
    durations: Vec<f64>,
}

impl BucketAccum {
    fn new() -> Self {
        Self {
            count: 0,
            failed_count: 0,
            durations: Vec::new(),
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn compute_latency(
    accum: &mut BucketAccum,
) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    accum
        .durations
        .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let avg = if accum.durations.is_empty() {
        0.0
    } else {
        accum.durations.iter().sum::<f64>() / accum.durations.len() as f64
    };
    (
        Some(avg),
        Some(percentile(&accum.durations, 50.0)),
        Some(percentile(&accum.durations, 95.0)),
        Some(percentile(&accum.durations, 99.0)),
    )
}

#[allow(clippy::cast_precision_loss)]
fn compute_error_rate(accum: &BucketAccum) -> f64 {
    if accum.count > 0 {
        accum.failed_count as f64 / accum.count as f64
    } else {
        0.0
    }
}

#[allow(clippy::cast_precision_loss)]
fn build_top_entries(
    top_counts: HashMap<String, u64>,
    total_count: u64,
    top_n: usize,
) -> Vec<AnalyticsTopEntry> {
    let mut entries: Vec<(String, u64)> = top_counts.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(top_n);
    entries
        .into_iter()
        .map(|(label, count)| {
            let pct = if total_count > 0 {
                (count as f64 / total_count as f64) * 100.0
            } else {
                0.0
            };
            AnalyticsTopEntry {
                label,
                count,
                percentage: pct,
            }
        })
        .collect()
}

#[async_trait]
impl<S: AuditStore + ?Sized + 'static> AnalyticsStore for InMemoryAnalytics<S> {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    async fn query_analytics(
        &self,
        query: &AnalyticsQuery,
    ) -> Result<AnalyticsResponse, AuditError> {
        let now = Utc::now();
        let from = query
            .from
            .unwrap_or_else(|| now - chrono::Duration::days(7));
        let to = query.to.unwrap_or(now);
        let top_n = query.top_n.unwrap_or(10);

        // Fetch all matching records in batches.
        let mut all_records = Vec::new();
        let batch_size = 1000u32;
        let mut offset = 0u32;

        loop {
            let audit_query = AuditQuery {
                namespace: query.namespace.clone(),
                tenant: query.tenant.clone(),
                provider: query.provider.clone(),
                action_type: query.action_type.clone(),
                outcome: query.outcome.clone(),
                from: Some(from),
                to: Some(to),
                limit: Some(batch_size),
                offset: Some(offset),
                ..Default::default()
            };

            let page = self.store.query(&audit_query).await?;
            let fetched = page.records.len();
            all_records.extend(page.records);

            if fetched < batch_size as usize {
                break;
            }
            offset += batch_size;
        }

        let total_count = all_records.len() as u64;

        // Group records into buckets.
        let mut bucket_map: HashMap<BucketKey, BucketAccum> = HashMap::new();
        let mut top_counts: HashMap<String, u64> = HashMap::new();

        for record in &all_records {
            let bucket_ts = truncate_to_interval(record.dispatched_at, query.interval);

            let group_label = query.group_by.as_deref().map(|dim| match dim {
                "action_type" => record.action_type.clone(),
                "outcome" => record.outcome.clone(),
                "namespace" => record.namespace.clone(),
                "tenant" => record.tenant.clone(),
                // "provider" and any unknown dimension default to provider.
                _ => record.provider.clone(),
            });

            let key = (bucket_ts, group_label);
            let accum = bucket_map.entry(key).or_insert_with(BucketAccum::new);
            accum.count += 1;
            accum.durations.push(record.duration_ms as f64);
            if record.outcome == "failed" {
                accum.failed_count += 1;
            }

            if query.metric == AnalyticsMetric::TopActionTypes {
                *top_counts.entry(record.action_type.clone()).or_insert(0) += 1;
            }
        }

        // Build buckets.
        let mut buckets: Vec<AnalyticsBucket> = bucket_map
            .into_iter()
            .map(|((timestamp, group), mut accum)| {
                let (avg_duration_ms, p50, p95, p99) = if query.metric == AnalyticsMetric::Latency {
                    compute_latency(&mut accum)
                } else {
                    (None, None, None, None)
                };

                let error_rate = if query.metric == AnalyticsMetric::ErrorRate {
                    Some(compute_error_rate(&accum))
                } else {
                    None
                };

                AnalyticsBucket {
                    timestamp,
                    count: accum.count,
                    group,
                    avg_duration_ms,
                    p50_duration_ms: p50,
                    p95_duration_ms: p95,
                    p99_duration_ms: p99,
                    error_rate,
                }
            })
            .collect();

        buckets.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then_with(|| a.group.cmp(&b.group))
        });

        let top_entries = if query.metric == AnalyticsMetric::TopActionTypes {
            build_top_entries(top_counts, total_count, top_n)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{AuditPage, AuditRecord};
    use crate::store::AuditStore;
    use chrono::Duration;
    use std::sync::{Arc, Mutex};

    /// A simple in-memory audit store for testing.
    struct MemoryAuditStore {
        records: Mutex<Vec<AuditRecord>>,
    }

    impl MemoryAuditStore {
        fn new() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
            }
        }

        fn add_record(&self, record: AuditRecord) {
            self.records.lock().unwrap().push(record);
        }
    }

    #[async_trait]
    impl AuditStore for MemoryAuditStore {
        async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
            self.records.lock().unwrap().push(entry);
            Ok(())
        }

        async fn get_by_action_id(
            &self,
            action_id: &str,
        ) -> Result<Option<AuditRecord>, AuditError> {
            let records = self.records.lock().unwrap();
            Ok(records.iter().find(|r| r.action_id == action_id).cloned())
        }

        async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
            let records = self.records.lock().unwrap();
            Ok(records.iter().find(|r| r.id == id).cloned())
        }

        async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
            let records = self.records.lock().unwrap();
            let mut filtered: Vec<AuditRecord> = records
                .iter()
                .filter(|r| {
                    query.namespace.as_ref().is_none_or(|ns| ns == &r.namespace)
                        && query.tenant.as_ref().is_none_or(|t| t == &r.tenant)
                        && query.provider.as_ref().is_none_or(|p| p == &r.provider)
                        && query
                            .action_type
                            .as_ref()
                            .is_none_or(|at| at == &r.action_type)
                        && query.outcome.as_ref().is_none_or(|o| o == &r.outcome)
                        && query.from.is_none_or(|from| r.dispatched_at >= from)
                        && query.to.is_none_or(|to| r.dispatched_at <= to)
                })
                .cloned()
                .collect();

            let total = filtered.len() as u64;
            let offset = query.effective_offset();
            let limit = query.effective_limit();

            filtered.sort_by(|a, b| b.dispatched_at.cmp(&a.dispatched_at));
            let records: Vec<AuditRecord> = filtered
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect();

            Ok(AuditPage {
                records,
                total,
                limit,
                offset,
            })
        }

        async fn cleanup_expired(&self) -> Result<u64, AuditError> {
            Ok(0)
        }
    }

    fn make_record(
        namespace: &str,
        tenant: &str,
        provider: &str,
        action_type: &str,
        outcome: &str,
        duration_ms: u64,
        dispatched_at: DateTime<Utc>,
    ) -> AuditRecord {
        AuditRecord {
            id: uuid::Uuid::now_v7().to_string(),
            action_id: uuid::Uuid::now_v7().to_string(),
            chain_id: None,
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            provider: provider.to_string(),
            action_type: action_type.to_string(),
            verdict: "allow".to_string(),
            matched_rule: None,
            outcome: outcome.to_string(),
            action_payload: None,
            verdict_details: serde_json::json!({}),
            outcome_details: serde_json::json!({}),
            metadata: serde_json::json!({}),
            dispatched_at,
            completed_at: dispatched_at + Duration::milliseconds(duration_ms as i64),
            duration_ms,
            expires_at: None,
            caller_id: String::new(),
            auth_method: String::new(),
            record_hash: None,
            previous_hash: None,
            sequence_number: None,
            attachment_metadata: Vec::new(),
        }
    }

    fn setup_store() -> Arc<MemoryAuditStore> {
        let store = Arc::new(MemoryAuditStore::new());
        let now = Utc::now();

        // Add records over the past 3 days.
        for i in 0..30 {
            let hours_ago = (i % 72) as i64;
            let ts = now - Duration::hours(hours_ago);
            let outcome = if i % 5 == 0 { "failed" } else { "executed" };
            let action_type = if i % 3 == 0 {
                "send_alert"
            } else if i % 3 == 1 {
                "create_ticket"
            } else {
                "send_notification"
            };
            let provider = if i % 2 == 0 { "webhook" } else { "email" };
            let duration = 50 + (i * 10);

            store.add_record(make_record(
                "default",
                "tenant-1",
                provider,
                action_type,
                outcome,
                duration,
                ts,
            ));
        }

        store
    }

    #[tokio::test]
    async fn test_volume_metric() {
        let store = setup_store();
        let analytics = InMemoryAnalytics::new(store);

        let query = AnalyticsQuery {
            metric: AnalyticsMetric::Volume,
            namespace: Some("default".to_string()),
            tenant: Some("tenant-1".to_string()),
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: None,
            top_n: None,
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        assert_eq!(result.metric, AnalyticsMetric::Volume);
        assert_eq!(result.total_count, 30);
        assert!(!result.buckets.is_empty());
        // All bucket counts should sum to total_count.
        let sum: u64 = result.buckets.iter().map(|b| b.count).sum();
        assert_eq!(sum, 30);
    }

    #[tokio::test]
    async fn test_outcome_breakdown() {
        let store = setup_store();
        let analytics = InMemoryAnalytics::new(store);

        let query = AnalyticsQuery {
            metric: AnalyticsMetric::OutcomeBreakdown,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: Some("outcome".to_string()),
            top_n: None,
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        assert_eq!(result.total_count, 30);
        // Should have group labels.
        let groups: Vec<&str> = result
            .buckets
            .iter()
            .filter_map(|b| b.group.as_deref())
            .collect();
        assert!(groups.contains(&"executed"));
        assert!(groups.contains(&"failed"));
    }

    #[tokio::test]
    async fn test_top_action_types() {
        let store = setup_store();
        let analytics = InMemoryAnalytics::new(store);

        let query = AnalyticsQuery {
            metric: AnalyticsMetric::TopActionTypes,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: None,
            top_n: Some(3),
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        assert!(!result.top_entries.is_empty());
        assert!(result.top_entries.len() <= 3);
        // Percentages should sum to ~100%.
        let pct_sum: f64 = result.top_entries.iter().map(|e| e.percentage).sum();
        assert!((pct_sum - 100.0).abs() < 0.1);
        // Entries should be sorted descending by count.
        for w in result.top_entries.windows(2) {
            assert!(w[0].count >= w[1].count);
        }
    }

    #[tokio::test]
    async fn test_latency_metric() {
        let store = setup_store();
        let analytics = InMemoryAnalytics::new(store);

        let query = AnalyticsQuery {
            metric: AnalyticsMetric::Latency,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: None,
            top_n: None,
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        for bucket in &result.buckets {
            assert!(bucket.avg_duration_ms.is_some());
            assert!(bucket.p50_duration_ms.is_some());
            assert!(bucket.p95_duration_ms.is_some());
            assert!(bucket.p99_duration_ms.is_some());
            // p50 <= p95 <= p99.
            let p50 = bucket.p50_duration_ms.unwrap();
            let p95 = bucket.p95_duration_ms.unwrap();
            let p99 = bucket.p99_duration_ms.unwrap();
            assert!(p50 <= p95 + f64::EPSILON);
            assert!(p95 <= p99 + f64::EPSILON);
        }
    }

    #[tokio::test]
    async fn test_error_rate_metric() {
        let store = setup_store();
        let analytics = InMemoryAnalytics::new(store);

        let query = AnalyticsQuery {
            metric: AnalyticsMetric::ErrorRate,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: None,
            top_n: None,
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        for bucket in &result.buckets {
            assert!(bucket.error_rate.is_some());
            let rate = bucket.error_rate.unwrap();
            assert!((0.0..=1.0).contains(&rate));
        }
    }

    #[tokio::test]
    async fn test_empty_results() {
        let store = Arc::new(MemoryAuditStore::new());
        let analytics = InMemoryAnalytics::new(store);

        let query = AnalyticsQuery {
            metric: AnalyticsMetric::Volume,
            namespace: Some("nonexistent".to_string()),
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Hourly,
            from: None,
            to: None,
            group_by: None,
            top_n: None,
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.buckets.is_empty());
        assert!(result.top_entries.is_empty());
    }

    #[tokio::test]
    async fn test_hourly_bucketing() {
        let store = Arc::new(MemoryAuditStore::new());
        let now = Utc::now();
        // Use two distinct hours in the past to avoid future-time filtering.
        let hour_ago = truncate_to_interval(now - Duration::hours(2), AnalyticsInterval::Hourly);
        let two_hours_ago = hour_ago - Duration::hours(1);

        store.add_record(make_record("ns", "t", "p", "a", "executed", 100, hour_ago));
        store.add_record(make_record(
            "ns",
            "t",
            "p",
            "a",
            "executed",
            200,
            hour_ago + Duration::minutes(30),
        ));
        store.add_record(make_record(
            "ns",
            "t",
            "p",
            "a",
            "executed",
            150,
            two_hours_ago,
        ));

        let analytics = InMemoryAnalytics::new(store);
        let query = AnalyticsQuery {
            metric: AnalyticsMetric::Volume,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Hourly,
            from: None,
            to: None,
            group_by: None,
            top_n: None,
        };

        let result = analytics.query_analytics(&query).await.unwrap();
        assert_eq!(result.total_count, 3);
        assert_eq!(result.buckets.len(), 2);
    }

    #[test]
    fn test_percentile_computation() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert!((percentile(&values, 50.0) - 5.5).abs() < 0.01);
        assert!((percentile(&values, 0.0) - 1.0).abs() < 0.01);
        assert!((percentile(&values, 100.0) - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_truncate_to_interval() {
        let dt = Utc.with_ymd_and_hms(2026, 2, 15, 14, 30, 45).unwrap();

        let hourly = truncate_to_interval(dt, AnalyticsInterval::Hourly);
        assert_eq!(hourly, Utc.with_ymd_and_hms(2026, 2, 15, 14, 0, 0).unwrap());

        let daily = truncate_to_interval(dt, AnalyticsInterval::Daily);
        assert_eq!(daily, Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap());

        let monthly = truncate_to_interval(dt, AnalyticsInterval::Monthly);
        assert_eq!(monthly, Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap());
    }
}
