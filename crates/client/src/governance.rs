//! Typed operator governance; management requests never retry automatically.
use crate::{ActeonClient, Error, PlatformOperation};
pub use acteon_core::{
    GovernanceChangeReceipt, GovernanceEffect, GovernanceIntervention,
    GovernanceInterventionRequest, GovernanceLimits, GovernanceManagementBounds,
    GovernancePermitDeclaration, GovernancePermitView, GovernanceRoute, GovernanceRouteView,
    GovernanceScopeView, ProviderExecutionHistory, ProviderHistoryReceipt,
    ProviderReconciliationCorrelation, ProviderReconciliationRequest,
    PublishGovernancePermitRequest,
};

impl ActeonClient {
    pub async fn governance(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<GovernanceScopeView, Error> {
        let value = self
            .platform_request(
                PlatformOperation::GovernanceInspect,
                &[],
                &[("namespace", namespace), ("tenant", tenant)],
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
    /// Inspect retained evidence under an explicitly granted history permission.
    pub async fn provider_execution_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<ProviderExecutionHistory, Error> {
        let value = self
            .platform_request(
                PlatformOperation::GovernanceProviderHistory,
                &[("execution_id", execution_id)],
                &[("namespace", namespace), ("tenant", tenant)],
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }

    /// Correlate retained work under independent reconciliation authority.
    pub async fn provider_reconciliation_correlation(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        ordinal: u32,
    ) -> Result<ProviderReconciliationCorrelation, Error> {
        let ordinal = ordinal.to_string();
        let value = self
            .platform_request(
                PlatformOperation::GovernanceReconciliationCorrelation,
                &[("execution_id", execution_id), ("ordinal", &ordinal)],
                &[("namespace", namespace), ("tenant", tenant)],
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
    /// Accept qualified finality without automatic retry or provider dispatch.
    pub async fn accept_provider_reconciliation(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        ordinal: u32,
        request: &ProviderReconciliationRequest,
    ) -> Result<ProviderHistoryReceipt, Error> {
        let ordinal = ordinal.to_string();
        let body =
            serde_json::to_value(request).map_err(|e| Error::Configuration(e.to_string()))?;
        let value = self
            .platform_request(
                PlatformOperation::GovernanceAcceptReconciliation,
                &[("execution_id", execution_id), ("ordinal", &ordinal)],
                &[("namespace", namespace), ("tenant", tenant)],
                Some(&body),
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }

    pub async fn publish_governance_permit(
        &self,
        request: &PublishGovernancePermitRequest,
    ) -> Result<GovernanceChangeReceipt, Error> {
        let body = serde_json::to_value(request)
            .map_err(|e| Error::Configuration(format!("governance request serialization: {e}")))?;
        let value = self
            .platform_request(
                PlatformOperation::GovernancePublishPermit,
                &[],
                &[],
                Some(&body),
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn intervene_governance(
        &self,
        request: &GovernanceInterventionRequest,
    ) -> Result<GovernanceChangeReceipt, Error> {
        let body = serde_json::to_value(request)
            .map_err(|e| Error::Configuration(format!("governance request serialization: {e}")))?;
        let value = self
            .platform_request(
                PlatformOperation::GovernanceIntervene,
                &[],
                &[],
                Some(&body),
            )
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
    ) -> (StatusCode, Json<serde_json::Value>) {
        assert_eq!(headers["authorization"], "Bearer operator-key");
        assert!(uri.query().unwrap().contains("namespace=prod"));
        assert!(uri.query().unwrap().contains("tenant=acme"));
        (StatusCode::OK, Json(f.wire["scope"].clone()))
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
    async fn management_wire_and_http_errors_are_preserved() {
        let fixture = Fixture {
            wire: serde_json::from_str(include_str!(
                "../../../clients/contract-fixtures/governance-management.json"
            ))
            .unwrap(),
            calls: Arc::new(Mutex::new(Vec::new())),
            status: Arc::new(AtomicU16::new(200)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/v1/governance", get(inspect))
            .route("/v1/governance/permits", post(change))
            .route("/v1/governance/changes", post(change))
            .with_state(fixture.clone());
        let server = tokio::spawn(axum::serve(listener, router).into_future());
        let client = crate::ActeonClientBuilder::new(url)
            .api_key("operator-key")
            .build()
            .unwrap();
        let scope = client.governance("prod", "acme").await.unwrap();
        assert_eq!(serde_json::to_value(scope).unwrap(), fixture.wire["scope"]);
        let publication: PublishGovernancePermitRequest =
            serde_json::from_value(fixture.wire["publication"].clone()).unwrap();
        let intervention: GovernanceInterventionRequest =
            serde_json::from_value(fixture.wire["intervention"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(
                client
                    .publish_governance_permit(&publication)
                    .await
                    .unwrap()
            )
            .unwrap(),
            fixture.wire["receipt"]
        );
        assert_eq!(
            serde_json::to_value(client.intervene_governance(&intervention).await.unwrap())
                .unwrap(),
            fixture.wire["receipt"]
        );
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            vec![
                fixture.wire["publication"].clone(),
                fixture.wire["intervention"].clone()
            ]
        );
        for status in [401, 403, 409, 503] {
            fixture.status.store(status, Ordering::SeqCst);
            let count = fixture.calls.lock().unwrap().len();
            assert!(
                matches!(client.intervene_governance(&intervention).await, Err(Error::Http { status: actual, .. }) if actual == status)
            );
            assert_eq!(fixture.calls.lock().unwrap().len(), count + 1);
        }
        server.abort();
    }
}

#[cfg(test)]
mod reconciliation_wire_tests {
    use super::*;
    #[test]
    fn reconciliation_models_preserve_correlation_proof_and_typed_finality() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/provider-reconciliation.json"
        ))
        .unwrap();
        let correlation: ProviderReconciliationCorrelation =
            serde_json::from_value(fixture["correlation"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(correlation).unwrap(),
            fixture["correlation"]
        );
        let request: ProviderReconciliationRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
        let mut untrusted = fixture["request"].clone();
        untrusted["verifier"] = "request-chosen".into();
        assert!(serde_json::from_value::<ProviderReconciliationRequest>(untrusted).is_err());
        let completed: ProviderHistoryReceipt =
            serde_json::from_value(fixture["receipt"].clone()).unwrap();
        assert!(matches!(
            completed.status,
            acteon_core::ProviderHistoryStatus::Completed {
                outcome: acteon_core::ActionOutcome::Executed(_)
            }
        ));
        let fenced: ProviderHistoryReceipt =
            serde_json::from_value(fixture["no_effect_receipt"].clone()).unwrap();
        assert!(matches!(
            fenced.status,
            acteon_core::ProviderHistoryStatus::Completed {
                outcome: acteon_core::ActionOutcome::Failed(_)
            }
        ));
        assert_eq!(
            PlatformOperation::GovernanceReconciliationCorrelation
                .descriptor()
                .0,
            "GET"
        );
        assert_eq!(
            PlatformOperation::GovernanceAcceptReconciliation
                .descriptor()
                .0,
            "POST"
        );
    }
}
