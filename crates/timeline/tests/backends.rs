//! Run with: cargo test -p acteon-timeline --test backends -- --ignored
mod cel_contract;
use acteon_timeline::{store::Projection, *};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;

#[allow(clippy::too_many_lines)]
async fn contract(store: Arc<dyn TimelineStore>, refresh: Option<(&reqwest::Client, String)>) {
    let scope = TimelineScope {
        namespace: "conformance".into(),
        tenant: "acme".into(),
    };
    let now = Utc::now();
    let event = TimelineEvent {
        scope: scope.clone(),
        source: "test".into(),
        event_id: "one".into(),
        revision: 1,
        first_ingested_at: now,
        updated_at: now,
        expires_at: now + chrono::Duration::days(7),
        event: EventBody {
            event_group: "old".into(),
            occurred_at: now,
            event_type: "deployment".into(),
            message: Some("Cluster Ready".into()),
            attributes: json!({"id":"550e8400-e29b-41d4-a716-446655440000","number":5}),
        },
    };
    let initial = Projection {
        scope: scope.clone(),
        source: event.source.clone(),
        event_id: event.event_id.clone(),
        revision: 1,
        expires_at: event.expires_at,
        event: Some(event),
    };
    store.apply(&initial).await.unwrap();
    if let Some((client, url)) = &refresh {
        client
            .post(url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    let q = TimelineQuery {
        equals: vec![AttributeEquals {
            path: "/id".into(),
            value: json!("550e8400-e29b-41d4-a716-446655440000"),
        }],
        text: Some(TextSearch {
            text: "CLUSTER ready".into(),
            path: None,
            mode: TextMode::All,
        }),
        ..Default::default()
    };
    assert_eq!(store.query(&scope, &q).await.unwrap().events.len(), 1);
    let cel = TimelineQuery {
        filter: Some(
            r#"attributes.id == "550e8400-e29b-41d4-a716-446655440000" || eventGroup == "old""#
                .into(),
        ),
        ..Default::default()
    };
    assert_eq!(store.query(&scope, &cel).await.unwrap().events.len(), 1);
    let other = TimelineScope {
        tenant: "other".into(),
        ..scope.clone()
    };
    assert_eq!(store.query(&other, &q).await.unwrap().events.len(), 0);
    let mut updated = initial.clone();
    updated.revision = 2;
    let e = updated.event.as_mut().unwrap();
    e.revision = 2;
    e.event.event_group = "new".into();
    e.event.attributes["id"] = json!("new id");
    store.apply(&updated).await.unwrap();
    store.apply(&initial).await.unwrap();
    if let Some((client, url)) = &refresh {
        client
            .post(url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    assert_eq!(store.query(&scope, &q).await.unwrap().events.len(), 0);
    assert!(store.query(&scope, &cel).await.unwrap().events.is_empty());
    assert_eq!(
        store
            .query(
                &scope,
                &TimelineQuery {
                    event_group: Some("old".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .events
            .len(),
        0
    );
    assert_eq!(
        store
            .get(&scope, "test", "one")
            .await
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    let mut deleted = updated.clone();
    deleted.revision = 3;
    deleted.event = None;
    store.apply(&deleted).await.unwrap();
    store.apply(&updated).await.unwrap();
    if let Some((client, url)) = &refresh {
        client
            .post(url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    assert!(store.get(&scope, "test", "one").await.unwrap().is_none());
    assert_eq!(
        store
            .query(&scope, &TimelineQuery::default())
            .await
            .unwrap()
            .events
            .len(),
        0
    );
    // Expiry clears payloads without removing version fences.
    let mut expired = initial;
    expired.event_id = "expired".into();
    expired.expires_at = now - chrono::Duration::seconds(1);
    let e = expired.event.as_mut().unwrap();
    e.event_id = "expired".into();
    e.expires_at = expired.expires_at;
    store.apply(&expired).await.unwrap();
    assert!(
        store
            .get(&scope, "test", "expired")
            .await
            .unwrap()
            .is_none()
    );
    store.cleanup(Utc::now()).await.unwrap();
    store.apply(&expired).await.unwrap();
    assert!(
        store
            .get(&scope, "test", "expired")
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
#[ignore = "requires Elasticsearch on localhost:9200"]
async fn elasticsearch_contract() {
    let client = reqwest::Client::new();
    let url = std::env::var("TIMELINE_ELASTICSEARCH_URL").unwrap_or("http://localhost:9200".into());
    let index = format!("timeline_test_{}", uuid::Uuid::new_v4().simple());
    let store = elasticsearch::ElasticsearchTimelineStore::new(client.clone(), &url, &index)
        .await
        .unwrap();
    cel_contract::run(&store, Some((&client, &format!("{url}/{index}/_refresh")))).await;
    contract(
        Arc::new(store),
        Some((&client, format!("{url}/{index}/_refresh"))),
    )
    .await;
    client
        .delete(format!("{url}/{index}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}
#[tokio::test]
#[ignore = "requires ClickHouse on localhost:8123"]
async fn clickhouse_contract() {
    let client = reqwest::Client::new();
    let url = std::env::var("TIMELINE_CLICKHOUSE_URL").unwrap_or("http://localhost:8123".into());
    let table = format!("timeline_test_{}", uuid::Uuid::new_v4().simple());
    let store = clickhouse::ClickHouseTimelineStore::new(client.clone(), &url, &table)
        .await
        .unwrap();
    cel_contract::run(&store, None).await;
    contract(Arc::new(store), None).await;
    client
        .post(url)
        .body(format!("DROP TABLE {table}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}
