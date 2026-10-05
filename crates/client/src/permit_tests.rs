use crate::{ActeonClient, PermitReference};
use acteon_core::Action;
use axum::{Json, Router, http::HeaderMap, routing::post};
use serde_json::{Value, json};
use std::{
    future::IntoFuture,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[tokio::test]
async fn permit_dispatch_preserves_auth_body_and_legacy_header_omission() {
    let fixture: Vec<PermitReference> = serde_json::from_str(include_str!(
        "../../../clients/contract-fixtures/execution-permits.json"
    ))
    .unwrap();
    let expected = serde_json::to_value(&fixture).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let handler = move |headers: HeaderMap, Json(body): Json<Value>| {
        let seen = seen.clone();
        let expected = expected.clone();
        async move {
            assert_eq!(headers["authorization"], "Bearer test-key");
            let index = seen.fetch_add(1, Ordering::SeqCst);
            if index == 0 {
                assert!(headers.get("x-acteon-execution-permits").is_none());
            } else {
                assert_eq!(
                    serde_json::from_str::<Value>(
                        headers["x-acteon-execution-permits"].to_str().unwrap()
                    )
                    .unwrap(),
                    expected
                );
            }
            let action = if body.is_array() { &body[0] } else { &body };
            assert_eq!(action["payload"], json!({"incident":42}));
            assert!(action.get("permits").is_none());
            Json(if body.is_array() {
                json!(["Deduplicated"])
            } else {
                json!("Deduplicated")
            })
        }
    };
    let router = Router::new()
        .route("/v1/dispatch", post(handler.clone()))
        .route("/v1/dispatch/batch", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = ActeonClient::builder(format!("http://{}", listener.local_addr().unwrap()))
        .api_key("test-key")
        .build()
        .unwrap();
    let server = tokio::spawn(axum::serve(listener, router).into_future());
    let action = Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"incident":42}),
    );
    client.dispatch(&action).await.unwrap();
    client
        .dispatch_with_permits(&action, &fixture)
        .await
        .unwrap();
    client
        .dispatch_batch_with_permits(&[action], &fixture)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.abort();
}

#[tokio::test]
async fn dispatch_refusals_preserve_http_status() {
    let router = Router::new()
        .route(
            "/v1/dispatch",
            post(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    Json(json!({"error":"permit refused"})),
                )
            }),
        )
        .route(
            "/v1/dispatch/batch",
            post(|| async {
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(json!([{"error":"replay conflict"}])),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = ActeonClient::new(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(axum::serve(listener, router).into_future());
    let action = Action::new("prod", "acme", "incident", "execute", json!({}));
    assert!(matches!(
        client.dispatch_with_permits(&action, &[]).await,
        Err(crate::Error::Http { status: 403, .. })
    ));
    assert!(matches!(
        client.dispatch_batch_with_permits(&[action], &[]).await,
        Err(crate::Error::Http { status: 409, .. })
    ));
    server.abort();
}
