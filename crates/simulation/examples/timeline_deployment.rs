//! Cluster deployment timeline: updates replace projections; CDC history survives deletion.
use acteon_core::timeline::*;
use acteon_timeline::{
    TimelineService, config::TimelineConfig, ledger::DiskLedger, memory::MemoryTimelineStore,
};
use std::sync::Arc;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        std::env::temp_dir().join(format!("acteon-timeline-demo-{}", uuid::Uuid::new_v4()));
    let service = TimelineService::new(
        Arc::new(MemoryTimelineStore::default()),
        Arc::new(DiskLedger::open(&directory, 16 * 1024 * 1024)?),
        Arc::new(acteon_state_memory::MemoryStateStore::new()),
        TimelineConfig::default(),
        None,
    )?;
    let scope = TimelineScope {
        namespace: "deployments".into(),
        tenant: "acme".into(),
    };
    let mut change = TimelineChange {
        schema_version: 1,
        change_id: "deploy-42-1".into(),
        source: "deployment-controller".into(),
        event_id: "deployment-42".into(),
        revision: 1,
        operation: ChangeOperation::Create,
        before: None,
        after: Some(EventBody {
            event_group: "cluster-eu-42".into(),
            occurred_at: chrono::Utc::now(),
            event_type: "cluster.deployment".into(),
            message: Some("Deployment started".into()),
            attributes: serde_json::json!({"deploymentId":"550e8400-e29b-41d4-a716-446655440000","status":"starting"}),
        }),
    };
    service.ingest(&scope, change.clone()).await?;
    change.revision = 2;
    change.change_id = "deploy-42-2".into();
    change.operation = ChangeOperation::Update;
    change.after.as_mut().unwrap().attributes["status"] = serde_json::json!("ready");
    service.ingest(&scope, change.clone()).await?;
    let query = TimelineQuery {
        filter: Some("eventType != \"ignored\"".into()),
        event_group: Some("cluster-eu-42".into()),
        ..Default::default()
    };
    let page = service.store.query(&scope, &query).await?;
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].revision, 2);
    println!("{}", serde_json::to_string_pretty(&page)?);
    change.revision = 3;
    change.change_id = "deploy-42-3".into();
    change.operation = ChangeOperation::Delete;
    change.after = None;
    service.ingest(&scope, change).await?;
    assert!(service.store.query(&scope, &query).await?.events.is_empty());
    assert_eq!(service.ledger.status(&scope).await?.records, 3);
    println!("Projection deleted; three CDC records retained in the independent ledger.");
    drop(service);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
