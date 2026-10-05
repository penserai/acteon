use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use acteon_core::Action;
use acteon_crypto::tls::LoadedTlsClientConfig;
use acteon_executor::catalog::QualifiedProviderCatalog;
use acteon_server::{config::ProviderConfig, provider_factory::StaticWebhook};
use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use serde_json::{Value, json};

fn config(url: &str) -> ProviderConfig {
    toml::from_str(&format!(
        "name = 'incident'\ntype = 'webhook'\nurl = '{url}'\ninternal_hosts = ['127.0.0.1']\n[headers]\nAuthorization = 'secret-token'"
    )).unwrap()
}

fn catalog(factory: &StaticWebhook) -> QualifiedProviderCatalog {
    QualifiedProviderCatalog::new_trusted(vec![
        factory
            .binding("prod", "acme", "execute", &[7; 32])
            .unwrap(),
    ])
    .unwrap()
}

#[test]
fn actual_configuration_changes_protected_binding_versions() {
    let baseline = config("http://127.0.0.1:9010/incident");
    let first = StaticWebhook::build(&baseline, None).unwrap();
    assert!(
        first
            .binding("prod", "acme", "execute", &[7; 32])
            .unwrap()
            .effect()
            .resources
            .iter()
            .any(|r| r.kind() == acteon_core::ResourceKind::Route)
    );
    let original = catalog(&first).definitions("prod", "acme").remove(0);
    assert_eq!(
        original,
        catalog(&StaticWebhook::build(&baseline, None).unwrap())
            .definitions("prod", "acme")
            .remove(0)
    );
    for variation in 0..4 {
        let mut changed = config("http://127.0.0.1:9010/incident");
        let tls = match variation {
            0 => {
                changed.url = Some("http://127.0.0.1:9010/other".into());
                None
            }
            1 => {
                changed
                    .headers
                    .insert("Authorization".into(), "rotated-token".into());
                None
            }
            2 => {
                changed.internal_hosts.push("localhost".into());
                None
            }
            _ => Some(Arc::new(
                LoadedTlsClientConfig::load(None, None, None, true).unwrap(),
            )),
        };
        let changed = catalog(&StaticWebhook::build(&changed, tls).unwrap())
            .definitions("prod", "acme")
            .remove(0);
        assert_ne!(original.revision, changed.revision);
        assert_ne!(original.effect, changed.effect);
        assert_eq!(original.endpoint, changed.endpoint);
    }
    let descriptor = serde_json::to_string(&original).unwrap();
    assert!(!descriptor.contains("secret-token"));
    assert!(!descriptor.contains("127.0.0.1"));
    assert!(first.binding("prod", "acme", "execute", &[0; 31]).is_err());
}

#[test]
fn canonical_configuration_and_ambiguous_headers() {
    let baseline = config("http://127.0.0.1:9010/incident");
    let mut reordered = config("http://127.0.0.1:9010/incident");
    reordered.internal_hosts = vec!["127.0.0.1".into(), "127.0.0.1.".into()];
    reordered.headers.clear();
    reordered
        .headers
        .insert("authorization".into(), "secret-token".into());
    assert_eq!(
        catalog(&StaticWebhook::build(&baseline, None).unwrap()).fingerprint(),
        catalog(&StaticWebhook::build(&reordered, None).unwrap()).fingerprint()
    );
    reordered
        .headers
        .insert("Authorization".into(), "other-secret".into());
    let error = StaticWebhook::build(&reordered, None).err().unwrap();
    assert!(error.contains("ambiguous"));
    assert!(!error.contains("other-secret"));
    reordered.headers.clear();
    reordered
        .headers
        .insert("authorization".into(), "bad\r\nvalue".into());
    assert!(StaticWebhook::build(&reordered, None).is_err());
    reordered.url = Some("http://169.254.169.254/credentials".into());
    assert!(StaticWebhook::build(&reordered, None).is_err());
}

#[tokio::test]
async fn real_instance_posts_only_to_its_configured_destination() {
    async fn receive(
        State(count): State<Arc<AtomicUsize>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        count.fetch_add(1, Ordering::SeqCst);
        assert_eq!(headers["authorization"], "secret-token");
        assert_eq!(body["payload"]["url"], "http://169.254.169.254/credentials");
        Json(json!({"accepted": true}))
    }
    let count = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/incident", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/incident", post(receive))
                .with_state(count.clone()),
        )
        .into_future(),
    );
    let factory = StaticWebhook::build(&config(&url), None).unwrap();
    let selected = factory.provider();
    let catalog = catalog(&factory);
    let action = Action::new(
        "prod",
        "acme",
        "original-before-fallback",
        "execute",
        json!({"url": "http://169.254.169.254/credentials"}),
    );
    assert!(catalog.resolve(&action, &selected).is_ok());
    let replacement = StaticWebhook::build(&config(&url), None)
        .unwrap()
        .provider();
    assert!(catalog.resolve(&action, &replacement).is_err());
    assert_eq!(
        selected.execute(&action).await.unwrap().status,
        acteon_core::ResponseStatus::Success
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn transport_failure_does_not_expose_destination_query_credentials() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/incident?token=private-query-credential",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);
    });
    let factory = StaticWebhook::build(&config(&url), None).unwrap();
    let action = Action::new("prod", "acme", "incident", "execute", json!({}));
    let error = factory
        .provider()
        .execute(&action)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("private-query-credential"));
    assert!(!error.contains("token="));
    assert!(!error.contains(&url));
    server.await.unwrap();
}

#[tokio::test]
async fn redirects_cannot_start_an_unregistered_second_request() {
    let count = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/redirect", listener.local_addr().unwrap());
    let router = Router::new()
        .route(
            "/redirect",
            post(|State(count): State<Arc<AtomicUsize>>| async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::response::Redirect::temporary("/escaped")
            }),
        )
        .route(
            "/escaped",
            axum::routing::any(|State(count): State<Arc<AtomicUsize>>| async move {
                count.fetch_add(1, Ordering::SeqCst);
                Json(json!({"escaped": true}))
            }),
        )
        .with_state(count.clone());
    let server = tokio::spawn(axum::serve(listener, router).into_future());
    let factory = StaticWebhook::build(&config(&url), None).unwrap();
    let action = Action::new("prod", "acme", "incident", "execute", json!({}));
    assert_eq!(
        factory.provider().execute(&action).await.unwrap().status,
        acteon_core::ResponseStatus::Failure
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}
