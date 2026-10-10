//! Typed operator governance; management requests never retry automatically.
use crate::{ActeonClient, Error, PlatformOperation};
pub use acteon_core::{
    FederationGrantCeilingDeclaration, FederationImportReceipt, FederationTrustDeclaration,
    FederationTrustView, GovernanceChangeReceipt, GovernanceEffect, GovernanceIntervention,
    GovernanceInterventionRequest, GovernanceLimits, GovernanceManagementBounds,
    GovernancePermitDeclaration, GovernancePermitView, GovernanceRegistryMutationReceipt,
    GovernanceRegistryMutationRequest, GovernanceRegistryProjection,
    GovernanceRegistryProjectionView, GovernanceRoute, GovernanceRouteView, GovernanceScopeView,
    ImportFederationEnvelopeRequest, InvokeFederatedAgentRequest, ProviderExecutionHistory,
    ProviderHistoryReceipt, ProviderReconciliationCorrelation, ProviderReconciliationRequest,
    PublishFederationTrustRequest, PublishGovernancePermitRequest, RevokeFederationTrustRequest,
    SignedFederationEnvelopeDeclaration, SignedFederationInvocationDeclaration, Task, TaskMessage,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationExportAttemptStatus {
    InFlight,
    Settled,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FederationExportDelivery {
    Registered,
    ImportUncertain,
    Imported {
        receipt: Box<FederationImportReceipt>,
    },
    InvokeUncertain {
        receipt: Box<FederationImportReceipt>,
    },
    Accepted {
        receipt: Box<FederationImportReceipt>,
        task: Box<Task>,
    },
    Rejected {
        stage: String,
        code: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationExportReceipt {
    pub attempt_id: String,
    pub attempt_status: FederationExportAttemptStatus,
    pub delivery_id: String,
    pub delivery: FederationExportDelivery,
}

#[derive(Serialize)]
struct FederationExportMessage<'a> {
    message: &'a TaskMessage,
}

impl ActeonClient {
    /// Inspect the current projection/version under independently granted agent bounds.
    pub async fn registry_projection(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
        projection: GovernanceRegistryProjection,
    ) -> Result<GovernanceRegistryProjectionView, Error> {
        let kind = match projection {
            GovernanceRegistryProjection::Agent => "agent",
            GovernanceRegistryProjection::Card => "card",
        };
        let value = self
            .platform_request(
                PlatformOperation::GovernanceRegistryProjection,
                &[("agent_id", agent_id)],
                &[
                    ("namespace", namespace),
                    ("tenant", tenant),
                    ("projection", kind),
                ],
                None,
            )
            .await?;
        let result: GovernanceRegistryProjectionView =
            serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))?;
        let resource = acteon_core::ResourceRef::new(
            acteon_core::ResourceKind::Agent,
            namespace,
            tenant,
            agent_id,
        )
        .map_err(|_| Error::Configuration("invalid registry agent identity".into()))?;
        if result.namespace != namespace
            || result.tenant != tenant
            || result.agent_id != agent_id
            || result.projection != projection
            || result.agent_resource != resource
            || (result.registry_revision == 0) != result.qualification_retired.is_none()
            || result.version == Some(0)
            || result.version.is_some() != result.value.is_some()
            || result
                .value
                .as_ref()
                .is_some_and(|value| !value.is_object())
        {
            return Err(Error::Deserialization(
                "registry observation identity or version mismatch".into(),
            ));
        }
        Ok(result)
    }
    /// Send one mutation. Preserve the exact request/change ID for explicit recovery.
    pub async fn mutate_registry(
        &self,
        request: &GovernanceRegistryMutationRequest,
    ) -> Result<GovernanceRegistryMutationReceipt, Error> {
        let body =
            serde_json::to_value(request).map_err(|e| Error::Configuration(e.to_string()))?;
        let value = self
            .platform_request(
                PlatformOperation::GovernanceMutateRegistry,
                &[],
                &[],
                Some(&body),
            )
            .await?;
        let receipt: GovernanceRegistryMutationReceipt =
            serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))?;
        if receipt.namespace != request.namespace
            || receipt.tenant != request.tenant
            || receipt.agent_id != request.agent_id
            || receipt.change_id != request.change_id
            || receipt.projection != request.projection
            || receipt.expected_registry_revision != request.expected_registry_revision
            || !receipt.applied
            || !receipt.delivery_complete
            || receipt.actor.is_empty()
            || receipt.input_digest.len() != 64
            || !receipt
                .input_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Deserialization(
                "unmatched or incomplete registry mutation receipt".into(),
            ));
        }
        Ok(receipt)
    }
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
    pub async fn publish_federation_trust(
        &self,
        request: &PublishFederationTrustRequest,
    ) -> Result<GovernanceChangeReceipt, Error> {
        let body = serde_json::to_value(request)
            .map_err(|e| Error::Configuration(format!("federation request serialization: {e}")))?;
        let value = self
            .platform_request(
                PlatformOperation::GovernancePublishFederationTrust,
                &[],
                &[],
                Some(&body),
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn revoke_federation_trust(
        &self,
        request: &RevokeFederationTrustRequest,
    ) -> Result<GovernanceChangeReceipt, Error> {
        let body = serde_json::to_value(request)
            .map_err(|e| Error::Configuration(format!("federation request serialization: {e}")))?;
        let value = self
            .platform_request(
                PlatformOperation::GovernanceRevokeFederationTrust,
                &[],
                &[],
                Some(&body),
            )
            .await?;
        serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))
    }
    /// Authenticate as a configured peer and retain imported authority without executing it.
    pub async fn import_federation_envelope(
        &self,
        namespace: &str,
        tenant: &str,
        envelope: SignedFederationEnvelopeDeclaration,
    ) -> Result<FederationImportReceipt, Error> {
        let body = serde_json::to_value(ImportFederationEnvelopeRequest { envelope })
            .map_err(|e| Error::Configuration(format!("federation request serialization: {e}")))?;
        let value = self
            .platform_request(
                PlatformOperation::FederationImportEnvelope,
                &[("namespace", namespace), ("tenant", tenant)],
                &[],
                Some(&body),
            )
            .await?;
        let receipt: FederationImportReceipt =
            serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))?;
        if receipt.namespace != namespace || receipt.tenant != tenant || receipt.execution_started {
            return Err(Error::Deserialization(
                "federation import receipt identity or execution state mismatch".into(),
            ));
        }
        Ok(receipt)
    }
    /// Invoke or replay an exact signed message as a configured federation peer.
    pub async fn invoke_federated_agent(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        invocation: SignedFederationInvocationDeclaration,
        message: TaskMessage,
    ) -> Result<Task, Error> {
        let body = serde_json::to_value(InvokeFederatedAgentRequest {
            invocation,
            message,
        })
        .map_err(|e| Error::Configuration(format!("federation request serialization: {e}")))?;
        let value = self
            .platform_request(
                PlatformOperation::FederationInvokeAgent,
                &[
                    ("namespace", namespace),
                    ("tenant", tenant),
                    ("agent", agent),
                ],
                &[],
                Some(&body),
            )
            .await?;
        let task: Task =
            serde_json::from_value(value).map_err(|e| Error::Deserialization(e.to_string()))?;
        if task.namespace != namespace || task.tenant != tenant || task.id.is_empty() {
            return Err(Error::Deserialization(
                "federated task receipt identity mismatch".into(),
            ));
        }
        Ok(task)
    }
    /// Submit one host-qualified export from an accepted local agent task.
    pub async fn export_federated_message(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        task_id: &str,
        profile: &str,
        message: &TaskMessage,
    ) -> Result<FederationExportReceipt, Error> {
        self.federation_export_request(
            PlatformOperation::FederationExportMessage,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )
        .await
    }
    /// Observe the original durable export without issuing network traffic.
    pub async fn observe_federated_export(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        task_id: &str,
        profile: &str,
        message: &TaskMessage,
    ) -> Result<FederationExportReceipt, Error> {
        self.federation_export_request(
            PlatformOperation::FederationObserveExportMessage,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )
        .await
    }
    /// Explicitly replay an exact request through a qualified idempotent peer.
    pub async fn replay_federated_export(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        task_id: &str,
        profile: &str,
        message: &TaskMessage,
    ) -> Result<FederationExportReceipt, Error> {
        self.federation_export_request(
            PlatformOperation::FederationReplayExportMessage,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn federation_export_request(
        &self,
        operation: PlatformOperation,
        namespace: &str,
        tenant: &str,
        agent: &str,
        task_id: &str,
        profile: &str,
        message: &TaskMessage,
    ) -> Result<FederationExportReceipt, Error> {
        let body = serde_json::to_value(FederationExportMessage { message })
            .map_err(|error| Error::Configuration(error.to_string()))?;
        let value = self
            .platform_request(
                operation,
                &[
                    ("namespace", namespace),
                    ("tenant", tenant),
                    ("agent", agent),
                    ("task_id", task_id),
                    ("profile", profile),
                ],
                &[],
                Some(&body),
            )
            .await?;
        let receipt: FederationExportReceipt = serde_json::from_value(value)
            .map_err(|error| Error::Deserialization(error.to_string()))?;
        if receipt.attempt_id.is_empty() || receipt.delivery_id.is_empty() {
            return Err(Error::Deserialization(
                "federation export receipt identity mismatch".into(),
            ));
        }
        Ok(receipt)
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
mod federation_wire_tests {
    use super::*;

    #[test]
    fn federation_management_and_inert_import_preserve_wire_contract() {
        let wire: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/federation-management.json"
        ))
        .unwrap();
        let publication: PublishFederationTrustRequest =
            serde_json::from_value(wire["publication"].clone()).unwrap();
        let revocation: RevokeFederationTrustRequest =
            serde_json::from_value(wire["revocation"].clone()).unwrap();
        let envelope: SignedFederationEnvelopeDeclaration =
            serde_json::from_value(wire["envelope"].clone()).unwrap();
        let receipt: FederationImportReceipt =
            serde_json::from_value(wire["import_receipt"].clone()).unwrap();
        assert!(!receipt.execution_started);
        assert_eq!(
            serde_json::to_value(publication).unwrap(),
            wire["publication"]
        );
        assert_eq!(
            serde_json::to_value(revocation).unwrap(),
            wire["revocation"]
        );
        assert_eq!(serde_json::to_value(envelope).unwrap(), wire["envelope"]);
        assert_eq!(
            serde_json::to_value(receipt).unwrap(),
            wire["import_receipt"]
        );
        assert_eq!(
            PlatformOperation::GovernancePublishFederationTrust
                .descriptor()
                .0,
            "POST"
        );
        assert_eq!(
            PlatformOperation::GovernanceRevokeFederationTrust
                .descriptor()
                .0,
            "POST"
        );
        assert_eq!(
            PlatformOperation::FederationImportEnvelope.descriptor().1,
            "/a2a/{namespace}/{tenant}/federation/envelopes:import"
        );
        let invocation = SignedFederationInvocationDeclaration {
            payload: "signed-payload".into(),
            signature: "a".repeat(128),
        };
        let message: TaskMessage = serde_json::from_value(serde_json::json!({
            "messageId": "message-1",
            "role": "user",
            "parts": [{"text": "inspect"}]
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(InvokeFederatedAgentRequest {
                invocation,
                message
            })
            .unwrap(),
            serde_json::json!({
                "invocation": {"payload": "signed-payload", "signature": "a".repeat(128)},
                "message": {"messageId": "message-1", "role": "user", "parts": [{"text": "inspect"}]}
            })
        );
        assert_eq!(
            PlatformOperation::FederationInvokeAgent.descriptor().1,
            "/a2a/{namespace}/{tenant}/federation/agents/{agent}/v1/message:send"
        );
        let export_receipt: FederationExportReceipt =
            serde_json::from_value(wire["export_receipt"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(export_receipt).unwrap(),
            wire["export_receipt"]
        );
        for (operation, suffix) in [
            (PlatformOperation::FederationExportMessage, "send"),
            (PlatformOperation::FederationObserveExportMessage, "observe"),
            (PlatformOperation::FederationReplayExportMessage, "replay"),
        ] {
            assert_eq!(operation.descriptor().0, "POST");
            assert!(
                operation
                    .descriptor()
                    .1
                    .ends_with(&format!("message:{suffix}"))
            );
        }
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

#[cfg(test)]
mod registry_tests {
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
            atomic::{AtomicU16, AtomicUsize, Ordering},
        },
    };
    #[derive(Clone)]
    struct Fixture {
        wire: Arc<Mutex<serde_json::Value>>,
        status: Arc<AtomicU16>,
        calls: Arc<AtomicUsize>,
    }
    async fn inspect(
        State(f): State<Fixture>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Json<serde_json::Value> {
        assert_eq!(headers["authorization"], "Bearer operator-key");
        assert_eq!(uri.path(), "/v1/governance/registry/maya");
        let query = uri.query().unwrap();
        for expected in ["namespace=prod", "tenant=acme", "projection=card"] {
            assert!(query.contains(expected));
        }
        f.calls.fetch_add(1, Ordering::SeqCst);
        Json(f.wire.lock().unwrap()["view"].clone())
    }
    async fn change(
        State(f): State<Fixture>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        assert_eq!(headers["authorization"], "Bearer operator-key");
        assert_eq!(body, f.wire.lock().unwrap()["request"]);
        f.calls.fetch_add(1, Ordering::SeqCst);
        (
            StatusCode::from_u16(f.status.load(Ordering::SeqCst)).unwrap(),
            Json(f.wire.lock().unwrap()["receipt"].clone()),
        )
    }
    #[tokio::test]
    async fn registry_helpers_preserve_requests_and_refuse_unmatched_or_incomplete_receipts() {
        let wire: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/governance-registry.json"
        ))
        .unwrap();
        let f = Fixture {
            wire: Arc::new(Mutex::new(wire.clone())),
            status: Arc::new(AtomicU16::new(200)),
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            crate::ActeonClientBuilder::new(format!("http://{}", listener.local_addr().unwrap()))
                .api_key("operator-key")
                .build()
                .unwrap();
        let router = Router::new()
            .route("/v1/governance/registry/{agent_id}", get(inspect))
            .route("/v1/governance/registry", post(change))
            .with_state(f.clone());
        let server = tokio::spawn(axum::serve(listener, router).into_future());
        assert_eq!(
            serde_json::to_value(
                client
                    .registry_projection("prod", "acme", "maya", GovernanceRegistryProjection::Card)
                    .await
                    .unwrap()
            )
            .unwrap(),
            wire["view"]
        );
        let request: GovernanceRegistryMutationRequest =
            serde_json::from_value(wire["request"].clone()).unwrap();
        for _ in 0..2 {
            assert_eq!(
                serde_json::to_value(client.mutate_registry(&request).await.unwrap()).unwrap(),
                wire["receipt"]
            );
        }
        for (field, value) in [
            ("delivery_complete", serde_json::json!(false)),
            ("applied", serde_json::json!(false)),
            ("change_id", serde_json::json!("other")),
            ("tenant", serde_json::json!("other")),
            ("input_digest", serde_json::json!("bad")),
        ] {
            let mut altered = wire["receipt"].clone();
            altered[field] = value;
            f.wire.lock().unwrap()["receipt"] = altered;
            let before = f.calls.load(Ordering::SeqCst);
            assert!(client.mutate_registry(&request).await.is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), before + 1);
        }
        for (field, value) in [
            ("tenant", serde_json::json!("other")),
            ("version", serde_json::json!(0)),
            ("qualification_retired", serde_json::Value::Null),
            ("registry_revision", serde_json::json!(0)),
            ("value", serde_json::json!("invalid")),
        ] {
            let mut changed = wire["view"].clone();
            changed[field] = value;
            f.wire.lock().unwrap()["view"] = changed;
            let before = f.calls.load(Ordering::SeqCst);
            assert!(
                client
                    .registry_projection("prod", "acme", "maya", GovernanceRegistryProjection::Card)
                    .await
                    .is_err()
            );
            assert_eq!(f.calls.load(Ordering::SeqCst), before + 1);
        }
        for status in [401, 403, 409, 503, 307] {
            f.status.store(status, Ordering::SeqCst);
            let before = f.calls.load(Ordering::SeqCst);
            assert!(
                matches!(client.mutate_registry(&request).await, Err(Error::Http {status:actual,..}) if actual==status)
            );
            assert_eq!(f.calls.load(Ordering::SeqCst), before + 1);
        }
        server.abort();
    }
}
