use acteon_timeline::{store::Projection, *};
use chrono::{TimeZone, Utc};
use serde_json::json;

#[allow(clippy::too_many_lines)]
pub async fn run(store: &dyn TimelineStore, refresh: Option<(&reqwest::Client, &str)>) {
    let scope = TimelineScope {
        namespace: "cel".into(),
        tenant: "acme".into(),
    };
    let now = Utc::now();
    for (id, attributes) in [
        (
            "a",
            json!({"region":"west","attempt":2,"enabled":true,"optional":null,"description":"Deployment FAILED","tags":["blue","green"],"nested":{"key/with~escape":"needle"},"items":[{"id":"hidden"}],"empty":[]}),
        ),
        (
            "b",
            json!({"region":"east","attempt":"2","enabled":false,"description":"Deployment ready","tags":["red"],"nested":{}}),
        ),
        (
            "c",
            json!({"region":"west","attempt":-2,"description":"Capacity pending","tags":"blue","quote":"O'Reilly"}),
        ),
    ] {
        let event = TimelineEvent {
            scope: scope.clone(),
            source: "controller".into(),
            event_id: id.into(),
            revision: 1,
            first_ingested_at: now,
            updated_at: now,
            expires_at: now + chrono::Duration::days(7),
            event: EventBody {
                event_group: "deployment-1".into(),
                event_type: "deployment".into(),
                occurred_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                message: Some("Deployment progress".into()),
                attributes,
            },
        };
        store
            .apply(&Projection {
                scope: scope.clone(),
                source: event.source.clone(),
                event_id: id.into(),
                revision: 1,
                expires_at: event.expires_at,
                event: Some(event),
            })
            .await
            .unwrap();
    }
    if let Some((client, url)) = refresh {
        client
            .post(url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    let cases: &[(&str, &[&str])] = &[
        (
            r#"attributes.region == "west" && (attributes.attempt == 2 || attributes.attempt == -2)"#,
            &["a", "c"],
        ),
        (
            r#"attributes.region in ["east", "west"] && attributes.enabled == true"#,
            &["a"],
        ),
        (r#"attributes.attempt == "2""#, &["b"]),
        (r#"attributes.region != "east""#, &["a", "c"]),
        (r#"!(attributes.region == "east")"#, &["a", "c"]),
        (
            r#"has(attributes.optional) && attributes.optional == null"#,
            &["a"],
        ),
        (r#"!has(attributes.optional)"#, &["b", "c"]),
        (r#"attributes.optional != null"#, &[]),
        (r#"!(attributes.missing == "x")"#, &[]),
        (r#"!(attributes.missing == "x" && false)"#, &["a", "b", "c"]),
        (r#"attributes.missing == "x" || true"#, &["a", "b", "c"]),
        (r#""blue" in attributes.tags"#, &["a"]),
        (r#"attributes.tags == "blue""#, &["c"]),
        (r#"!("blue" in attributes.tags)"#, &["b"]),
        (r#"has(attributes.empty)"#, &["a"]),
        (r#"has(attributes.items.id)"#, &[]),
        (
            r#"attributes.nested["key/with~escape"] == "needle""#,
            &["a"],
        ),
        (r#"attributes.quote == "O'Reilly""#, &["c"]),
        (
            r#"textMatch(attributes.description, "FAILED deployment")"#,
            &["a"],
        ),
        (
            r#"textAny(attributes.description, "failed pending")"#,
            &["a", "c"],
        ),
        (r#"textMatch("DEPLOYMENT progress")"#, &["a", "b", "c"]),
        (r#"textMatch(attributes.description, "fail")"#, &[]),
        (
            r#"eventGroup == "deployment-1" && source == "controller" && eventType == "deployment" && eventId in ["a", "c"]"#,
            &["a", "c"],
        ),
        (r#"eventId in []"#, &[]),
        (
            r#"occurredAt >= timestamp("2026-01-01T00:00:00Z") && occurredAt < timestamp("2026-01-01T00:00:00.000000001Z")"#,
            &["a", "b", "c"],
        ),
        (
            r#"occurredAt == timestamp("2025-12-31T19:00:00-05:00")"#,
            &["a", "b", "c"],
        ),
        (r#"occurredAt != timestamp("2026-01-01T00:00:00Z")"#, &[]),
        (r#"eventGroup == "x' OR 1=1 --\\""#, &[]),
        (
            r#"false || attributes.region == "west" && attributes.attempt == 2"#,
            &["a"],
        ),
        ("false", &[]),
        ("!(attributes.missing in [])", &[]),
        ("!(attributes.region in [])", &["a", "b", "c"]),
    ];
    for (filter, expected) in cases {
        let query = TimelineQuery {
            filter: Some((*filter).into()),
            ascending: Some(true),
            ..Default::default()
        };
        let result = store
            .query(&scope, &query)
            .await
            .unwrap_or_else(|e| panic!("{filter}: {e}"));
        let ids: Vec<_> = result.events.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(&ids, expected, "filter: {filter}");
        let other = TimelineScope {
            tenant: "other".into(),
            ..scope.clone()
        };
        assert!(store.query(&other, &query).await.unwrap().events.is_empty());
    }
    for filter in [
        "",
        "attributes.attempt > 1",
        "namespace == \"other\"",
        "attributes.region == attributes.quote",
        "attributes.tags[0] == \"blue\"",
        "attributes.region.matches(\".*\")",
        "unknown()",
        "eventGroup == 3",
        "occurredAt > timestamp(\"bad\")",
        "true ? true : false",
        "textMatch(\"!!!\")",
        "has(source)",
        r#"attributes.region == "\u0077est""#,
        r#"attributes.region == "\q""#,
    ] {
        let result = store
            .query(
                &scope,
                &TimelineQuery {
                    filter: Some(filter.into()),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            matches!(result, Err(Error::Invalid(_))),
            "must reject {filter}"
        );
    }
    for filter in [
        "true ".repeat(1000),
        format!("{}true{}", "(".repeat(80), ")".repeat(80)),
        std::iter::repeat_n("true", 150)
            .collect::<Vec<_>>()
            .join(" || "),
    ] {
        assert!(matches!(
            store
                .query(
                    &scope,
                    &TimelineQuery {
                        filter: Some(filter),
                        ..Default::default()
                    }
                )
                .await,
            Err(Error::Invalid(_))
        ));
    }
    let mut query = TimelineQuery {
        filter: Some("attributes.region == \"west\"".into()),
        ascending: Some(true),
        limit: Some(1),
        ..Default::default()
    };
    let first = store.query(&scope, &query).await.unwrap();
    assert_eq!(first.events[0].event_id, "a");
    query.cursor = first.next_cursor;
    assert!(query.cursor.is_some());
    let second = store.query(&scope, &query).await.unwrap();
    assert_eq!(second.events[0].event_id, "c");
    assert!(second.next_cursor.is_none());
    query.filter = Some("true".into());
    assert!(matches!(
        store.query(&scope, &query).await,
        Err(Error::Invalid(_))
    ));
    query.cursor = None;
    query.equals = vec![AttributeEquals {
        path: "/attempt".into(),
        value: json!(2),
    }];
    assert_eq!(
        store.query(&scope, &query).await.unwrap().events[0].event_id,
        "a"
    );
    query.event_group = Some("other".into());
    assert!(store.query(&scope, &query).await.unwrap().events.is_empty());
}
