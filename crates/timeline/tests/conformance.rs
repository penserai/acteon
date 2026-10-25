use acteon_timeline::{config::TimelineConfig, ledger::DiskLedger, memory::MemoryTimelineStore, *};
use chrono::Utc;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn scope() -> TimelineScope {
    TimelineScope {
        namespace: "deployments".into(),
        tenant: "acme".into(),
    }
}
fn change(id: &str, revision: u64) -> TimelineChange {
    TimelineChange {
        schema_version: 1,
        change_id: format!("{id}-{revision}"),
        source: "deploy".into(),
        event_id: id.into(),
        revision,
        operation: ChangeOperation::Update,
        before: None,
        after: Some(EventBody {
            event_group: "cluster-1".into(),
            occurred_at: Utc::now(),
            event_type: "deployment".into(),
            message: Some("Cluster deployment completed".into()),
            attributes: json!({"customId":"550e8400-e29b-41d4-a716-446655440000","attempt":1,"nested":{"key/with~escape":"needle"},"labels":["blue","green"]}),
        }),
    }
}
fn service(dir: &tempfile::TempDir) -> TimelineService {
    TimelineService::new(
        Arc::new(MemoryTimelineStore::default()),
        Arc::new(DiskLedger::open(dir.path(), 10_000_000).unwrap()),
        Arc::new(acteon_state_memory::MemoryStateStore::new()),
        TimelineConfig::default(),
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn latest_version_exact_search_deletion_and_scope_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let s = scope();
    let first = change("a", 1);
    svc.ingest(&s, first.clone()).await.unwrap();
    assert_eq!(
        svc.ingest(&s, first.clone()).await.unwrap().disposition,
        ChangeDisposition::Duplicate
    );
    let exact = TimelineQuery {
        equals: vec![AttributeEquals {
            path: "/customId".into(),
            value: json!("550e8400-e29b-41d4-a716-446655440000"),
        }],
        ..Default::default()
    };
    assert_eq!(svc.store.query(&s, &exact).await.unwrap().events.len(), 1);
    let partial = TimelineQuery {
        equals: vec![AttributeEquals {
            path: "/customId".into(),
            value: json!("550e8400"),
        }],
        ..Default::default()
    };
    assert_eq!(svc.store.query(&s, &partial).await.unwrap().events.len(), 0);
    let mut newer = change("a", 2);
    newer.after.as_mut().unwrap().attributes["customId"] = json!("replacement");
    svc.ingest(&s, newer.clone()).await.unwrap();
    assert_eq!(svc.store.query(&s, &exact).await.unwrap().events.len(), 0);
    assert_eq!(
        svc.ingest(&s, first).await.unwrap().disposition,
        ChangeDisposition::Stale
    );
    let event = svc.store.get(&s, "deploy", "a").await.unwrap().unwrap();
    assert_eq!(event.revision, 2);
    assert_eq!(
        (event.expires_at - event.first_ingested_at).num_seconds(),
        604_800
    );
    let other = TimelineScope {
        tenant: "acmecorp".into(),
        ..s.clone()
    };
    assert_eq!(
        svc.store
            .query(&other, &TimelineQuery::default())
            .await
            .unwrap()
            .events
            .len(),
        0
    );
    let mut conflict = newer;
    conflict.after.as_mut().unwrap().message = Some("conflict".into());
    assert!(matches!(
        svc.ingest(&s, conflict).await,
        Err(Error::Conflict)
    ));
    let mut delete = change("a", 3);
    delete.operation = ChangeOperation::Delete;
    delete.after = None;
    svc.ingest(&s, delete).await.unwrap();
    assert!(svc.store.get(&s, "deploy", "a").await.unwrap().is_none());
    assert_eq!(
        svc.ingest(&s, change("a", 4)).await.unwrap().disposition,
        ChangeDisposition::Terminal
    );
    svc.replay_local(&s).await.unwrap();
    assert_eq!(
        svc.store
            .query(&s, &TimelineQuery::default())
            .await
            .unwrap()
            .events
            .len(),
        0
    );
    assert_eq!(svc.ledger.status(&s).await.unwrap().pending, 0);
    assert!(svc.ledger.status(&s).await.unwrap().records >= 4);
}
#[tokio::test]
async fn typed_paths_full_text_and_cursor_scope() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let s = scope();
    for id in ["a", "b", "c"] {
        svc.ingest(&s, change(id, 1)).await.unwrap();
    }
    for (path, value) in [
        ("/attempt", json!(1)),
        ("/nested/key~1with~0escape", json!("needle")),
        ("/labels", json!("green")),
    ] {
        let q = TimelineQuery {
            equals: vec![AttributeEquals {
                path: path.into(),
                value,
            }],
            ..Default::default()
        };
        assert_eq!(svc.store.query(&s, &q).await.unwrap().events.len(), 3);
    }
    let q = TimelineQuery {
        equals: vec![AttributeEquals {
            path: "/attempt".into(),
            value: json!("1"),
        }],
        ..Default::default()
    };
    assert_eq!(svc.store.query(&s, &q).await.unwrap().events.len(), 0);
    let mut q = TimelineQuery {
        text: Some(TextSearch {
            text: "CLUSTER completed".into(),
            path: None,
            mode: TextMode::All,
        }),
        limit: Some(2),
        ..Default::default()
    };
    let first = svc.store.query(&s, &q).await.unwrap();
    assert_eq!(first.events.len(), 2);
    q.cursor = first.next_cursor;
    assert!(q.cursor.is_some());
    let second = svc.store.query(&s, &q).await.unwrap();
    assert_eq!(second.events.len(), 1);
    assert_ne!(first.events[0].event_id, second.events[0].event_id);
    let other = TimelineScope {
        tenant: "other".into(),
        ..s
    };
    assert!(svc.store.query(&other, &q).await.is_err());
}
struct FailingStore {
    fail: AtomicBool,
    inner: MemoryTimelineStore,
}
#[async_trait::async_trait]
impl TimelineStore for FailingStore {
    async fn apply(&self, p: &store::Projection) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            Err(Error::Storage("offline".into()))
        } else {
            self.inner.apply(p).await
        }
    }
    async fn get(
        &self,
        s: &TimelineScope,
        source: &str,
        id: &str,
    ) -> Result<Option<TimelineEvent>> {
        self.inner.get(s, source, id).await
    }
    async fn query(&self, s: &TimelineScope, q: &TimelineQuery) -> Result<TimelinePage> {
        self.inner.query(s, q).await
    }
    async fn cleanup(&self, now: chrono::DateTime<Utc>) -> Result<u64> {
        self.inner.cleanup(now).await
    }
}
#[tokio::test]
async fn failed_projection_is_pinned_and_recovered_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(acteon_state_memory::MemoryStateStore::new());
    let store = Arc::new(FailingStore {
        fail: AtomicBool::new(true),
        inner: MemoryTimelineStore::default(),
    });
    let config = TimelineConfig::default();
    let input = change("a", 1);
    {
        let svc = TimelineService::new(
            store.clone(),
            Arc::new(DiskLedger::open(dir.path(), 10_000_000).unwrap()),
            state.clone(),
            config.clone(),
            None,
        )
        .unwrap();
        assert!(svc.ingest(&scope(), input.clone()).await.is_err());
        assert_eq!(svc.ledger.status(&scope()).await.unwrap().pending, 1);
        assert_eq!(
            svc.ledger
                .cleanup(Utc::now() + chrono::Duration::days(2))
                .await
                .unwrap(),
            0
        );
    }
    store.fail.store(false, Ordering::SeqCst);
    let svc = TimelineService::new(
        store,
        Arc::new(DiskLedger::open(dir.path(), 10_000_000).unwrap()),
        state,
        config,
        None,
    )
    .unwrap();
    assert_eq!(svc.recover_all().await.unwrap(), 1);
    assert!(
        svc.store
            .get(&scope(), "deploy", "a")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        svc.ingest(&scope(), input).await.unwrap().disposition,
        ChangeDisposition::Duplicate
    );
    assert_eq!(svc.ledger.status(&scope()).await.unwrap().records, 1);
    assert_eq!(
        svc.ledger
            .cleanup(Utc::now() + chrono::Duration::days(2))
            .await
            .unwrap(),
        1
    );
}
#[tokio::test]
async fn concurrent_changes_converge_to_highest_revision() {
    let dir = tempfile::tempdir().unwrap();
    let svc = Arc::new(service(&dir));
    let mut jobs = vec![];
    for revision in 1..=20 {
        let svc = svc.clone();
        jobs.push(tokio::spawn(async move {
            svc.ingest(&scope(), change("a", revision)).await
        }));
    }
    for job in jobs {
        job.await.unwrap().unwrap();
    }
    assert_eq!(
        svc.store
            .get(&scope(), "deploy", "a")
            .await
            .unwrap()
            .unwrap()
            .revision,
        20
    );
}
#[test]
fn strict_metadata_and_limits() {
    let c = change("a", 1);
    let mut value = serde_json::to_value(c).unwrap();
    value["expiresAt"] = json!("2030-01-01T00:00:00Z");
    assert!(serde_json::from_value::<TimelineChange>(value).is_err());
    let mut c = change("a", 0);
    assert!(search::validate_change(&c).is_err());
    c.revision = 1;
    c.after.as_mut().unwrap().attributes = json!(vec![1]);
    assert!(search::validate_change(&c).is_err());
}

#[tokio::test]
async fn expired_payloads_cannot_return_on_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryTimelineStore::default());
    let svc = TimelineService::new(
        store,
        Arc::new(DiskLedger::open(dir.path(), 10_000_000).unwrap()),
        Arc::new(acteon_state_memory::MemoryStateStore::new()),
        TimelineConfig {
            ttl_seconds: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    svc.ingest(&scope(), change("a", 1)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(
        svc.ingest(&scope(), change("a", 2))
            .await
            .unwrap()
            .disposition,
        ChangeDisposition::Terminal
    );
    svc.replay_local(&scope()).await.unwrap();
    assert_eq!(
        svc.store
            .query(&scope(), &TimelineQuery::default())
            .await
            .unwrap()
            .events
            .len(),
        0
    );
}
#[test]
fn debezium_snapshots_deletes_and_tombstones() {
    let mapping = debezium::DebeziumMapping {
        event_group: "/group".into(),
        occurred_at: "/time".into(),
        event_type: "/kind".into(),
        message: None,
        attributes: None,
        generation: "v1".into(),
    };
    let raw = json!({"payload":{"source":{"connector":"postgresql","lsn":42},"op":"r","before":null,"after":{"group":"g","time":"2026-01-01T00:00:00Z","kind":"deployment"}}});
    let change = mapping
        .decode("pg", &json!({"id":1}), &raw, "events", 0, 0)
        .unwrap()
        .unwrap();
    assert_eq!(change.revision, 1);
    assert_eq!(change.operation, ChangeOperation::Snapshot);
    assert_eq!(change.source, "pg@v1");
    assert!(
        mapping
            .decode(
                "pg",
                &json!({"id":1}),
                &serde_json::Value::Null,
                "events",
                0,
                1
            )
            .unwrap()
            .is_none()
    );
    let deleted =
        json!({"source":{"connector":"postgresql"},"op":"d","before":{"id":1},"after":null});
    let deleted = mapping
        .decode("pg", &json!({"id":1}), &deleted, "events", 0, 2)
        .unwrap()
        .unwrap();
    assert_eq!(deleted.event_id, change.event_id);
    assert!(deleted.after.is_none());
}

#[cfg(feature = "archive")]
#[tokio::test]
async fn archive_failure_prevents_projection_and_acceptance() {
    use acteon_timeline::ledger::{LedgerArchiveSink, LedgerEntry};
    struct Archive {
        fail: AtomicBool,
    }
    #[async_trait::async_trait]
    impl LedgerArchiveSink for Archive {
        async fn publish(&self, _: &LedgerEntry) -> Result<()> {
            if self.fail.load(Ordering::SeqCst) {
                Err(Error::Storage("archive unavailable".into()))
            } else {
                Ok(())
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let archive = Arc::new(Archive {
        fail: AtomicBool::new(true),
    });
    let config = TimelineConfig {
        archive: Some(config::ArchiveConfig {
            bucket: "test".into(),
            prefix: "test".into(),
            endpoint: None,
            region: None,
            ttl_seconds: DEFAULT_ARCHIVE_TTL_SECONDS,
        }),
        ..Default::default()
    };
    let svc = TimelineService::new(
        Arc::new(MemoryTimelineStore::default()),
        Arc::new(DiskLedger::open(dir.path(), 10_000_000).unwrap()),
        Arc::new(acteon_state_memory::MemoryStateStore::new()),
        config,
        Some(archive.clone()),
    )
    .unwrap();
    let c = change("a", 1);
    assert!(svc.ingest(&scope(), c.clone()).await.is_err());
    assert!(
        svc.store
            .get(&scope(), "deploy", "a")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(svc.ledger.status(&scope()).await.unwrap().pending, 1);
    archive.fail.store(false, Ordering::SeqCst);
    svc.recover_all().await.unwrap();
    assert_eq!(
        svc.ingest(&scope(), c).await.unwrap().disposition,
        ChangeDisposition::Duplicate
    );
    assert_eq!(svc.ledger.status(&scope()).await.unwrap().pending, 0);
}
#[cfg(feature = "bus")]
#[tokio::test]
#[ignore = "requires Kafka on localhost:9092"]
async fn kafka_acknowledges_persisted_events_and_halts_invalid_input() {
    use acteon_bus::{
        BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, StartOffset, SubscriptionConfig,
    };
    let backend = KafkaBackend::new(&KafkaBusConfig::default()).unwrap();
    let mut topic = acteon_core::Topic::new(
        format!("timeline-{}", uuid::Uuid::new_v4().simple()),
        "test",
        "tenant",
    );
    topic.partitions = 1;
    topic.replication_factor = 1;
    backend.create_topic(&topic).await.unwrap();
    let name = topic.kafka_topic_name();
    let group = format!("timeline-{}", uuid::Uuid::new_v4());
    backend
        .produce(
            BusMessage::new(&name, serde_json::to_value(change("a", 1)).unwrap()).with_key("a"),
        )
        .await
        .unwrap();
    backend
        .produce(BusMessage::new(&name, json!({"invalid":true})))
        .await
        .unwrap();
    let mut source = config::SourceConfig {
        poison_policy: config::PoisonPolicy::Halt,
        namespace: scope().namespace,
        tenant: scope().tenant,
        source: "deploy".into(),
        topic: name.clone(),
        consumer_group: group.clone(),
        debezium: None,
    };
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let mut subscription = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        bus::consume_one(&svc, subscription.as_mut(), &source, &scope()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        svc.store
            .get(&scope(), "deploy", "a")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(svc.ledger.status(&scope()).await.unwrap().pending, 0);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            bus::consume_one(&svc, subscription.as_mut(), &source, &scope())
        )
        .await
        .unwrap()
        .is_err()
    );
    let lag = backend.consumer_lag(&name, &group).await.unwrap();
    assert_eq!(lag.len(), 1);
    assert_eq!(lag[0].committed, 0);
    assert_eq!(lag[0].lag, 1);
    assert_eq!(lag[0].high_water_mark, 2);
    drop(subscription);
    source.poison_policy = config::PoisonPolicy::Quarantine;
    let mut subscription = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        bus::consume_one(&svc, subscription.as_mut(), &source, &scope()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(svc.ledger.status(&scope()).await.unwrap().records, 2);
    assert_eq!(
        svc.store
            .query(&scope(), &TimelineQuery::default())
            .await
            .unwrap()
            .events
            .len(),
        1
    );
    assert_eq!(
        backend.consumer_lag(&name, &group).await.unwrap()[0].committed,
        1
    );
    drop(subscription);
    backend.delete_topic(&name).await.unwrap();
}

#[tokio::test]
async fn quarantine_is_durable_idempotent_and_never_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let raw = json!({"not":"a CDC envelope"});
    let transport =
        json!({"topic":"events","partition":0,"offset":42,"rejection":"decode_failure"});
    svc.quarantine(&scope(), "source", raw.clone(), transport.clone())
        .await
        .unwrap();
    svc.quarantine(&scope(), "source", raw, transport)
        .await
        .unwrap();
    assert_eq!(svc.ledger.status(&scope()).await.unwrap().records, 1);
    assert_eq!(
        svc.ledger.read(&scope(), None, 10).await.unwrap()[0].disposition,
        ChangeDisposition::Quarantined
    );
    assert_eq!(
        svc.store
            .query(&scope(), &TimelineQuery::default())
            .await
            .unwrap()
            .events
            .len(),
        0
    );
    let mut input = change("fake", 1);
    input.operation = ChangeOperation::Rejected;
    input.after = None;
    assert!(svc.ingest(&scope(), input).await.is_err());
}
