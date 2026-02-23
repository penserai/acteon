//! Simulation of the analytics API in Acteon.
//!
//! Dispatches ~60 actions with varying providers, outcomes, and action types,
//! then queries each metric type and prints the results.
//!
//! Run with: `cargo run -p acteon-simulation --example analytics_simulation`

use std::sync::Arc;
use std::time::Instant;

use acteon_audit::InMemoryAnalytics;
use acteon_audit::analytics::AnalyticsStore;
use acteon_audit::record::AuditRecord;
use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::analytics::{
    AnalyticsInterval, AnalyticsMetric, AnalyticsQuery, AnalyticsResponse,
};
use chrono::{Duration, Utc};

fn make_record(
    namespace: &str,
    tenant: &str,
    provider: &str,
    action_type: &str,
    outcome: &str,
    duration_ms: u64,
    dispatched_at: chrono::DateTime<chrono::Utc>,
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
        completed_at: dispatched_at + chrono::Duration::milliseconds(duration_ms as i64),
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

fn print_response(label: &str, resp: &AnalyticsResponse) {
    println!("\n{}", "=".repeat(60));
    println!("  {label}");
    println!("{}", "=".repeat(60));
    println!(
        "  Metric: {:?} | Interval: {:?} | Total: {}",
        resp.metric, resp.interval, resp.total_count
    );
    println!(
        "  Range: {} -> {}",
        resp.from.format("%Y-%m-%d %H:%M"),
        resp.to.format("%Y-%m-%d %H:%M")
    );
    println!("  Buckets: {}", resp.buckets.len());
    for bucket in &resp.buckets {
        let group = bucket
            .group
            .as_deref()
            .map(|g| format!(" [{g}]"))
            .unwrap_or_default();
        let extras = [
            bucket.avg_duration_ms.map(|v| format!("avg={v:.1}ms")),
            bucket.p50_duration_ms.map(|v| format!("p50={v:.1}ms")),
            bucket.p95_duration_ms.map(|v| format!("p95={v:.1}ms")),
            bucket.p99_duration_ms.map(|v| format!("p99={v:.1}ms")),
            bucket.error_rate.map(|v| format!("err={:.1}%", v * 100.0)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");

        println!(
            "    {} | count={}{} {}",
            bucket.timestamp.format("%Y-%m-%d %H:%M"),
            bucket.count,
            group,
            extras
        );
    }
    if !resp.top_entries.is_empty() {
        println!("  Top entries:");
        for entry in &resp.top_entries {
            println!(
                "    {}: {} ({:.1}%)",
                entry.label, entry.count, entry.percentage
            );
        }
    }
}

#[tokio::main]
async fn main() {
    println!("Acteon Analytics Simulation");
    println!("===========================\n");

    let start = Instant::now();

    // Create audit store and populate with test data.
    let audit_store = Arc::new(MemoryAuditStore::new());
    let now = Utc::now();

    let providers = ["webhook", "email", "slack"];
    let action_types = [
        "send_alert",
        "create_ticket",
        "send_notification",
        "update_status",
    ];
    let outcomes = ["executed", "executed", "executed", "executed", "failed"]; // 20% failure rate

    for i in 0..60 {
        let hours_ago = (i * 2) % 168; // spread over 7 days
        let ts = now - Duration::hours(hours_ago as i64);
        let provider = providers[i % providers.len()];
        let action_type = action_types[i % action_types.len()];
        let outcome = outcomes[i % outcomes.len()];
        let duration = 50 + (i * 7) % 500;

        let record = make_record(
            "notifications",
            "tenant-1",
            provider,
            action_type,
            outcome,
            duration as u64,
            ts,
        );
        audit_store.record(record).await.unwrap();
    }

    println!("Populated {} audit records", 60);

    let analytics = InMemoryAnalytics::new(audit_store as Arc<dyn AuditStore>);

    // 1. Volume metric
    let resp = analytics
        .query_analytics(&AnalyticsQuery {
            metric: AnalyticsMetric::Volume,
            namespace: Some("notifications".to_string()),
            tenant: Some("tenant-1".to_string()),
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: None,
            top_n: None,
        })
        .await
        .unwrap();
    print_response("1. Volume (Daily)", &resp);

    // 2. Outcome breakdown
    let resp = analytics
        .query_analytics(&AnalyticsQuery {
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
        })
        .await
        .unwrap();
    print_response("2. Outcome Breakdown (grouped by outcome)", &resp);

    // 3. Top action types
    let resp = analytics
        .query_analytics(&AnalyticsQuery {
            metric: AnalyticsMetric::TopActionTypes,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Weekly,
            from: None,
            to: None,
            group_by: None,
            top_n: Some(5),
        })
        .await
        .unwrap();
    print_response("3. Top Action Types (top 5)", &resp);

    // 4. Latency percentiles
    let resp = analytics
        .query_analytics(&AnalyticsQuery {
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
        })
        .await
        .unwrap();
    print_response("4. Latency Percentiles (Daily)", &resp);

    // 5. Error rate
    let resp = analytics
        .query_analytics(&AnalyticsQuery {
            metric: AnalyticsMetric::ErrorRate,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Hourly,
            from: Some(now - Duration::hours(24)),
            to: Some(now),
            group_by: None,
            top_n: None,
        })
        .await
        .unwrap();
    print_response("5. Error Rate (Hourly, last 24h)", &resp);

    // 6. Volume grouped by provider
    let resp = analytics
        .query_analytics(&AnalyticsQuery {
            metric: AnalyticsMetric::Volume,
            namespace: None,
            tenant: None,
            provider: None,
            action_type: None,
            outcome: None,
            interval: AnalyticsInterval::Daily,
            from: None,
            to: None,
            group_by: Some("provider".to_string()),
            top_n: None,
        })
        .await
        .unwrap();
    print_response("6. Volume grouped by Provider", &resp);

    println!("\n\nSimulation completed in {:?}", start.elapsed());
}
