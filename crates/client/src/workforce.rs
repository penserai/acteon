//! Typed workforce management; wire declarations do not establish caller authority.
use crate::{ActeonClient, Error, PlatformOperation};
pub use acteon_core::workforce::*;

impl ActeonClient {
    pub async fn workforce(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<WorkforceScopeView, Error> {
        let value = self
            .platform_request(
                PlatformOperation::WorkforceInspect,
                &[],
                &[("namespace", namespace), ("tenant", tenant)],
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn change_workforce(
        &self,
        request: &WorkforceChangeRequest,
    ) -> Result<acteon_core::GovernanceChangeReceipt, Error> {
        let body = serde_json::to_value(request)
            .map_err(|e| Error::Configuration(format!("workforce request serialization: {e}")))?;
        let value = self
            .platform_request(PlatformOperation::WorkforceChange, &[], &[], Some(&body))
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        routing::{get, post},
    };
    use std::{
        future::IntoFuture,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU16, Ordering},
        },
    };
    #[derive(Clone)]
    struct Fixture {
        wire: serde_json::Value,
        calls: Arc<Mutex<Vec<serde_json::Value>>>,
        status: Arc<AtomicU16>,
    }
    async fn inspect(
        State(f): State<Fixture>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Json<serde_json::Value> {
        assert_eq!(headers["authorization"], "Bearer operator-key");
        assert!(uri.query().unwrap().contains("namespace=prod"));
        assert!(uri.query().unwrap().contains("tenant=acme"));
        Json(f.wire["scope"].clone())
    }
    async fn change(
        State(f): State<Fixture>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        assert_eq!(headers["authorization"], "Bearer operator-key");
        f.calls.lock().unwrap().push(body);
        (
            StatusCode::from_u16(f.status.load(Ordering::SeqCst)).unwrap(),
            Json(f.wire["receipt"].clone()),
        )
    }
    #[tokio::test]
    async fn all_workforce_variants_preserve_wire_and_refusals() {
        let fixture = Fixture {
            wire: serde_json::from_str(include_str!(
                "../../../clients/contract-fixtures/workforce-management.json"
            ))
            .unwrap(),
            calls: Arc::new(Mutex::new(vec![])),
            status: Arc::new(AtomicU16::new(200)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/workforce", get(inspect))
                    .route("/v1/workforce/changes", post(change))
                    .with_state(fixture.clone()),
            )
            .into_future(),
        );
        let client = crate::ActeonClientBuilder::new(url)
            .api_key("operator-key")
            .build()
            .unwrap();
        assert_eq!(
            serde_json::to_value(client.workforce("prod", "acme").await.unwrap()).unwrap(),
            fixture.wire["scope"]
        );
        for wire in fixture.wire["changes"].as_array().unwrap() {
            let request: WorkforceChangeRequest = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(&request).unwrap(), *wire);
            assert_eq!(
                serde_json::to_value(client.change_workforce(&request).await.unwrap()).unwrap(),
                fixture.wire["receipt"]
            );
            assert_eq!(fixture.calls.lock().unwrap().last().unwrap(), wire);
        }
        let request: WorkforceChangeRequest =
            serde_json::from_value(fixture.wire["changes"][0].clone()).unwrap();
        for status in [401, 403, 409, 503] {
            fixture.status.store(status, Ordering::SeqCst);
            let before = fixture.calls.lock().unwrap().len();
            assert!(
                matches!(client.change_workforce(&request).await,Err(Error::Http {status:actual,..}) if actual==status)
            );
            assert_eq!(fixture.calls.lock().unwrap().len(), before + 1);
        }
        server.abort();
    }
}
