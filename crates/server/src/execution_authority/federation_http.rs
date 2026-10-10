//! Guarded HTTP delivery for exact signed Acteon federation requests.

use std::{sync::Arc, time::Duration};

use acteon_core::{FederationImportReceipt, Task};
use acteon_crypto::{ExposeSecret, SecretString};
use acteon_executor::federation_transport::{
    FederationCapabilityDisposition, FederationCapabilityEvidence, FederationCapabilityRequest,
    FederationImportDisposition, FederationImportRequest, FederationInvokeDisposition,
    FederationInvokeRequest, FederationTransportAdapter, FederationTransportError,
};
use acteon_governance::federation::{
    FederationCapabilityRequirement, SignedFederationCapabilityCard,
    validate_federation_capability_verifying_key, verify_federation_capability_card,
};
use acteon_http::{GuardedClient, OutboundPolicy};
use acteon_time::Clock;
use async_trait::async_trait;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 2_048;

/// Host-installed adapter for another Acteon domain. The bearer credential and
/// outbound policy are process configuration and never enter the durable
/// delivery record. Acteon's exact import and invocation IDs make replay of the
/// same signed requests idempotent; a different payload under either ID is a
/// conflict at the receiver.
pub struct ActeonFederationHttpAdapter {
    binding_digest: String,
    revision: String,
    credential: Arc<dyn FederationCredentialProvider>,
    capability: FederationHttpCapabilityQualification,
    clock: Arc<dyn Clock>,
    client: GuardedClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationCredentialError {
    Refused,
    Unavailable,
}

#[async_trait]
pub trait FederationCredentialProvider: Send + Sync {
    fn scheme(&self) -> &str;
    fn revision(&self) -> &str;
    async fn credential(&self) -> Result<SecretString, FederationCredentialError>;
}

pub struct StaticFederationCredentialProvider {
    credential: SecretString,
}

impl StaticFederationCredentialProvider {
    pub fn new(credential: SecretString) -> Result<Self, FederationCredentialError> {
        if credential.expose_secret().is_empty() {
            return Err(FederationCredentialError::Refused);
        }
        Ok(Self { credential })
    }
}

#[async_trait]
impl FederationCredentialProvider for StaticFederationCredentialProvider {
    fn scheme(&self) -> &'static str {
        "bearer"
    }

    fn revision(&self) -> &'static str {
        "static-bearer-v1"
    }

    async fn credential(&self) -> Result<SecretString, FederationCredentialError> {
        Ok(self.credential.clone())
    }
}

#[derive(Clone)]
pub struct FederationHttpCapabilityQualification {
    pub key_id: String,
    pub verifying_key: String,
    pub max_clock_skew_ms: i64,
    pub requirement: FederationCapabilityRequirement,
}

pub struct FederationHttpAdapterInstallation {
    pub credential: Arc<dyn FederationCredentialProvider>,
    pub capability: FederationHttpCapabilityQualification,
    pub clock: Arc<dyn Clock>,
    pub policy: OutboundPolicy,
    pub timeout: Duration,
}

impl ActeonFederationHttpAdapter {
    pub fn new_trusted(
        binding_digest: &str,
        revision: &str,
        installation: FederationHttpAdapterInstallation,
    ) -> Result<Self, FederationTransportError> {
        Self::new_trusted_with_builder(
            binding_digest,
            revision,
            installation,
            reqwest::Client::builder(),
        )
    }

    pub fn new_trusted_with_builder(
        binding_digest: &str,
        revision: &str,
        installation: FederationHttpAdapterInstallation,
        builder: reqwest::ClientBuilder,
    ) -> Result<Self, FederationTransportError> {
        let FederationHttpAdapterInstallation {
            credential,
            capability,
            clock,
            policy,
            timeout,
        } = installation;
        if !valid_digest(binding_digest)
            || !valid_text(revision, 120)
            || !valid_text(credential.revision(), 120)
            || credential.scheme() != capability.requirement.authentication_scheme
            || !valid_text(&capability.key_id, 255)
            || validate_federation_capability_verifying_key(&capability.verifying_key).is_err()
            || capability.requirement.validate().is_err()
            || !(0..=86_400_000).contains(&capability.max_clock_skew_ms)
            || timeout.is_zero()
        {
            return Err(FederationTransportError::Invalid);
        }
        let client = GuardedClient::from_builder(builder.timeout(timeout), policy, false)
            .map_err(|_| FederationTransportError::Invalid)?;
        Ok(Self {
            binding_digest: binding_digest.into(),
            revision: revision.into(),
            credential,
            capability,
            clock,
            client,
        })
    }
}

#[async_trait]
impl FederationTransportAdapter for ActeonFederationHttpAdapter {
    fn revision(&self) -> &str {
        &self.revision
    }

    fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    fn exact_replay_idempotent(&self) -> bool {
        true
    }

    async fn negotiate(
        &self,
        request: FederationCapabilityRequest<'_>,
    ) -> Result<FederationCapabilityDisposition, FederationTransportError> {
        let endpoint = route_endpoint(
            request.endpoint,
            &[
                "a2a",
                request.namespace,
                request.tenant,
                "federation",
                "agents",
                request.agent_id,
                "capabilities",
            ],
        )?;
        let Ok(builder) = self.client.request(reqwest::Method::GET, &endpoint) else {
            return Ok(FederationCapabilityDisposition::Rejected {
                code: "federation_capability_endpoint_refused".into(),
            });
        };
        let credential = match self.credential.credential().await {
            Ok(credential) => credential,
            Err(FederationCredentialError::Refused) => {
                return Ok(FederationCapabilityDisposition::Rejected {
                    code: "federation_credential_refused".into(),
                });
            }
            Err(FederationCredentialError::Unavailable) => {
                return Ok(FederationCapabilityDisposition::Unavailable);
            }
        };
        let Ok(response) = builder.bearer_auth(credential.expose_secret()).send().await else {
            return Ok(FederationCapabilityDisposition::Unavailable);
        };
        let status = response.status();
        if status.is_success() {
            let Ok(raw) = read_bounded(response, MAX_RESPONSE_BYTES).await else {
                return Ok(FederationCapabilityDisposition::Rejected {
                    code: "federation_capability_invalid".into(),
                });
            };
            let Ok(signed) = serde_json::from_slice::<SignedFederationCapabilityCard>(&raw) else {
                return Ok(FederationCapabilityDisposition::Rejected {
                    code: "federation_capability_invalid".into(),
                });
            };
            let Ok(card) = verify_federation_capability_card(
                &signed,
                &self.capability.key_id,
                &self.capability.verifying_key,
                self.clock.now().timestamp_millis(),
                self.capability.max_clock_skew_ms,
                &self.capability.requirement,
            ) else {
                return Ok(FederationCapabilityDisposition::Rejected {
                    code: "federation_capability_denied".into(),
                });
            };
            return Ok(FederationCapabilityDisposition::Accepted(
                FederationCapabilityEvidence {
                    card_digest: signed
                        .digest()
                        .map_err(|_| FederationTransportError::Invalid)?,
                    key_id: card.key_id,
                    issuer_epoch: card.issuer_epoch,
                    expires_at_ms: card.expires_at_ms,
                    protocol_revision: card.protocol_revision,
                    authentication_scheme: self
                        .capability
                        .requirement
                        .authentication_scheme
                        .clone(),
                },
            ));
        }
        if known_capability_rejection(status.as_u16()) {
            return Ok(FederationCapabilityDisposition::Rejected {
                code: rejection_code(response, "federation_capability", status.as_u16()).await,
            });
        }
        Ok(FederationCapabilityDisposition::Unavailable)
    }

    async fn import(
        &self,
        request: FederationImportRequest<'_>,
    ) -> Result<FederationImportDisposition, FederationTransportError> {
        let envelope = request
            .envelope
            .envelope()
            .map_err(|_| FederationTransportError::Invalid)?;
        let endpoint = route_endpoint(
            request.endpoint,
            &[
                "a2a",
                envelope.agent_resource.namespace(),
                envelope.agent_resource.tenant(),
                "federation",
                "envelopes:import",
            ],
        )?;
        let Ok(builder) = self.client.post(&endpoint) else {
            return Ok(FederationImportDisposition::Rejected {
                code: "federation_destination_refused".into(),
            });
        };
        let credential = match self.credential.credential().await {
            Ok(credential) => credential,
            Err(FederationCredentialError::Refused) => {
                return Ok(FederationImportDisposition::Rejected {
                    code: "federation_credential_refused".into(),
                });
            }
            Err(FederationCredentialError::Unavailable) => {
                return Ok(FederationImportDisposition::NotSent);
            }
        };
        let Ok(response) = builder
            .bearer_auth(credential.expose_secret())
            .json(&serde_json::json!({"envelope": request.envelope}))
            .send()
            .await
        else {
            return Ok(FederationImportDisposition::Uncertain);
        };
        let status = response.status();
        if status.is_success() {
            return Ok(match read_bounded(response, MAX_RESPONSE_BYTES).await {
                Ok(raw) => serde_json::from_slice::<FederationImportReceipt>(&raw)
                    .map_or(FederationImportDisposition::Uncertain, |receipt| {
                        FederationImportDisposition::Accepted(Box::new(receipt))
                    }),
                Err(()) => FederationImportDisposition::Uncertain,
            });
        }
        if known_rejection(status.as_u16()) {
            return Ok(FederationImportDisposition::Rejected {
                code: rejection_code(response, "federation_import", status.as_u16()).await,
            });
        }
        Ok(FederationImportDisposition::Uncertain)
    }

    async fn invoke(
        &self,
        request: FederationInvokeRequest<'_>,
    ) -> Result<FederationInvokeDisposition, FederationTransportError> {
        let invocation = request
            .invocation
            .invocation()
            .map_err(|_| FederationTransportError::Invalid)?;
        if serde_json::to_value(&invocation.message)
            .map_err(|_| FederationTransportError::Invalid)?
            != serde_json::to_value(request.message)
                .map_err(|_| FederationTransportError::Invalid)?
        {
            return Err(FederationTransportError::Conflict);
        }
        let endpoint = route_endpoint(
            request.endpoint,
            &[
                "a2a",
                request.namespace,
                request.tenant,
                "federation",
                "agents",
                request.agent_id,
                "v1",
                "message:send",
            ],
        )?;
        let Ok(builder) = self.client.post(&endpoint) else {
            return Ok(FederationInvokeDisposition::Rejected {
                code: "federation_destination_refused".into(),
            });
        };
        let credential = match self.credential.credential().await {
            Ok(credential) => credential,
            Err(FederationCredentialError::Refused) => {
                return Ok(FederationInvokeDisposition::Rejected {
                    code: "federation_credential_refused".into(),
                });
            }
            Err(FederationCredentialError::Unavailable) => {
                return Ok(FederationInvokeDisposition::NotSent);
            }
        };
        let Ok(response) = builder
            .bearer_auth(credential.expose_secret())
            .json(&serde_json::json!({
                "invocation": request.invocation,
                "message": request.message,
            }))
            .send()
            .await
        else {
            return Ok(FederationInvokeDisposition::Uncertain);
        };
        let status = response.status();
        if status.is_success() {
            return Ok(match read_bounded(response, MAX_RESPONSE_BYTES).await {
                Ok(raw) => serde_json::from_slice::<Task>(&raw)
                    .map_or(FederationInvokeDisposition::Uncertain, |task| {
                        FederationInvokeDisposition::Accepted(Box::new(task))
                    }),
                Err(()) => FederationInvokeDisposition::Uncertain,
            });
        }
        if known_rejection(status.as_u16()) {
            return Ok(FederationInvokeDisposition::Rejected {
                code: rejection_code(response, "federation_invoke", status.as_u16()).await,
            });
        }
        Ok(FederationInvokeDisposition::Uncertain)
    }
}

fn route_endpoint(origin: &str, segments: &[&str]) -> Result<String, FederationTransportError> {
    let mut url = reqwest::Url::parse(origin).map_err(|_| FederationTransportError::Invalid)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(FederationTransportError::Refused);
    }
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| FederationTransportError::Refused)?;
        path.clear();
        for segment in segments {
            if !valid_text(segment, 1_024) || matches!(*segment, "." | "..") {
                return Err(FederationTransportError::Invalid);
            }
            path.push(segment);
        }
    }
    Ok(url.into())
}

fn known_rejection(status: u16) -> bool {
    matches!(status, 400 | 401 | 403 | 404 | 409 | 429)
}

fn known_capability_rejection(status: u16) -> bool {
    matches!(status, 400 | 401 | 403 | 404 | 409)
}

async fn rejection_code(response: reqwest::Response, prefix: &str, status: u16) -> String {
    read_bounded(response, MAX_ERROR_BYTES)
        .await
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|code| valid_text(code, 1_024))
        .unwrap_or_else(|| format!("{prefix}_http_{status}"))
}

async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{
        ImportFederationEnvelopeRequest, InvokeFederatedAgentRequest, PrincipalIdentity,
        PrincipalKind, ResourceKind, ResourceRef, TaskMessage, TaskRole,
    };
    use acteon_executor::federation_transport::{FederationImportRequest, FederationInvokeRequest};
    use acteon_governance::{
        RootBudgetLimits,
        context::AcceptedEffect,
        federation::{
            FederationCapabilityCard, FederationCapabilityRequirement, FederationCapabilitySigner,
            FederationEnvelope, FederationEnvelopeSigner, FederationImportReference,
            FederationInvocation,
        },
    };
    use acteon_time::ManualClock;
    use axum::{
        Json, Router,
        http::HeaderMap,
        routing::{get, post},
    };

    fn signed_requests() -> (
        acteon_governance::federation::SignedFederationEnvelope,
        acteon_governance::federation::SignedFederationInvocation,
        TaskMessage,
    ) {
        let source = PrincipalIdentity::new("source-agent", PrincipalKind::Agent).unwrap();
        let target = PrincipalIdentity::new("remote-agent", PrincipalKind::Agent).unwrap();
        let agent = ResourceRef::new(ResourceKind::Agent, "remote", "tenant", "responder").unwrap();
        let ingress = AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: vec![agent.clone()],
        };
        let signer = FederationEnvelopeSigner::from_secret([41; 32]);
        let envelope = signer
            .sign(&FederationEnvelope::new(
                "export-http-1",
                "trust-http-1",
                1,
                "source.example",
                "remote.example",
                "source-key-1",
                1,
                90,
                100,
                5_000,
                source,
                target,
                agent.clone(),
                "b".repeat(64),
                "respond",
                ingress.clone(),
                vec![ingress],
                RootBudgetLimits {
                    max_units: 1,
                    max_concurrent: 1,
                    deadline_ms: 4_900,
                },
                1,
            ))
            .unwrap();
        let message = TaskMessage::text("http-message-1", TaskRole::User, "inspect incident");
        let invocation = signer
            .sign_invocation(&FederationInvocation::new(
                "http-invocation-1",
                FederationImportReference {
                    id: "export-http-1".into(),
                    envelope_digest: envelope.digest().unwrap(),
                },
                message.clone(),
                100,
                4_000,
            ))
            .unwrap();
        (envelope, invocation, message)
    }

    #[tokio::test]
    async fn guarded_adapter_calls_exact_federation_routes_with_host_credential() {
        let (envelope, invocation, message) = signed_requests();
        let import_envelope = envelope.clone();
        let invoke_message = message.clone();
        let capability_signer = FederationCapabilitySigner::from_secret([24; 32]);
        let capability_key = capability_signer.verifying_key_hex();
        let capability_card = capability_signer
            .sign(&FederationCapabilityCard::new(
                "remote.example",
                "source.example",
                "remote-capability-v1",
                1,
                90,
                1_000,
                ResourceRef::new(ResourceKind::Agent, "remote", "tenant", "responder").unwrap(),
                "b".repeat(64),
                vec!["respond".into()],
                "acteon-federation-v1",
                true,
                true,
                vec!["bearer".into()],
            ))
            .unwrap();
        let app = Router::new()
            .route(
                "/a2a/remote/tenant/federation/agents/responder/capabilities",
                get(move |headers: HeaderMap| {
                    let card = capability_card.clone();
                    async move {
                        assert_eq!(headers["authorization"], "Bearer federation-secret");
                        Json(card)
                    }
                }),
            )
            .route(
                "/a2a/remote/tenant/federation/envelopes:import",
                post(
                    move |headers: HeaderMap, Json(body): Json<ImportFederationEnvelopeRequest>| {
                        let envelope = import_envelope.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer federation-secret");
                            assert_eq!(body.envelope.payload, envelope.payload);
                            let decoded = envelope.envelope().unwrap();
                            Json(FederationImportReceipt {
                                namespace: "remote".into(),
                                tenant: "tenant".into(),
                                import_id: decoded.id,
                                envelope_digest: envelope.digest().unwrap(),
                                trust_id: decoded.trust_id,
                                trust_revision: decoded.trust_revision,
                                issuer_domain: decoded.issuer_domain,
                                source: decoded.source,
                                target: decoded.target,
                                agent_resource: decoded.agent_resource,
                                binding_digest: decoded.binding_digest,
                                skill: decoded.skill,
                                root_id: "remote-root-http-1".into(),
                                expires_at_ms: decoded.expires_at_ms,
                                accepted_at_ms: 110,
                                execution_started: false,
                            })
                        }
                    },
                ),
            )
            .route(
                "/a2a/remote/tenant/federation/agents/responder/v1/message:send",
                post(
                    move |headers: HeaderMap, Json(body): Json<InvokeFederatedAgentRequest>| {
                        let message = invoke_message.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer federation-secret");
                            assert_eq!(body.message.message_id, message.message_id);
                            let mut task = Task::new_at(
                                "remote-task-http-1",
                                "remote",
                                "tenant",
                                chrono::DateTime::from_timestamp_millis(110).unwrap(),
                            );
                            let mut initial = message;
                            initial.task_id = Some(task.id.clone());
                            task.append_history_at(
                                initial,
                                chrono::DateTime::from_timestamp_millis(110).unwrap(),
                            )
                            .unwrap();
                            Json(task)
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let adapter = ActeonFederationHttpAdapter::new_trusted(
            &"a".repeat(64),
            "acteon-federation-http-v1",
            FederationHttpAdapterInstallation {
                credential: Arc::new(
                    StaticFederationCredentialProvider::new(SecretString::new(
                        "federation-secret".into(),
                    ))
                    .unwrap(),
                ),
                capability: FederationHttpCapabilityQualification {
                    key_id: "remote-capability-v1".into(),
                    verifying_key: capability_key,
                    max_clock_skew_ms: 20,
                    requirement: FederationCapabilityRequirement {
                        issuer_domain: "remote.example".into(),
                        audience: "source.example".into(),
                        minimum_issuer_epoch: 1,
                        agent_resource: ResourceRef::new(
                            ResourceKind::Agent,
                            "remote",
                            "tenant",
                            "responder",
                        )
                        .unwrap(),
                        binding_digest: "b".repeat(64),
                        skill: "respond".into(),
                        protocol_revision: "acteon-federation-v1".into(),
                        require_exact_replay: true,
                        require_peer_finality: true,
                        authentication_scheme: "bearer".into(),
                    },
                },
                clock: Arc::new(ManualClock::new(
                    chrono::DateTime::from_timestamp_millis(100).unwrap(),
                )),
                policy: OutboundPolicy {
                    internal_hosts: vec!["127.0.0.1".into()],
                },
                timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let origin = format!("http://{address}");

        let capability = adapter
            .negotiate(FederationCapabilityRequest {
                endpoint: &origin,
                namespace: "remote",
                tenant: "tenant",
                agent_id: "responder",
            })
            .await
            .unwrap();
        assert!(matches!(
            capability,
            FederationCapabilityDisposition::Accepted(_)
        ));

        let imported = adapter
            .import(FederationImportRequest {
                endpoint: &origin,
                envelope: &envelope,
            })
            .await
            .unwrap();
        assert!(matches!(imported, FederationImportDisposition::Accepted(_)));
        let invoked = adapter
            .invoke(FederationInvokeRequest {
                endpoint: &origin,
                namespace: "remote",
                tenant: "tenant",
                agent_id: "responder",
                invocation: &invocation,
                message: &message,
            })
            .await
            .unwrap();
        assert!(matches!(invoked, FederationInvokeDisposition::Accepted(_)));
        server.abort();
    }

    #[test]
    fn route_builder_refuses_non_origin_and_encodes_segments() {
        assert!(route_endpoint("https://peer.example/prefix", &["a2a"]).is_err());
        assert!(route_endpoint("https://user@peer.example", &["a2a"]).is_err());
        assert_eq!(
            route_endpoint("https://peer.example", &["a2a", "tenant one"]).unwrap(),
            "https://peer.example/a2a/tenant%20one"
        );
        assert!(known_capability_rejection(403));
        assert!(!known_capability_rejection(429));
        assert!(!known_capability_rejection(503));
    }
}
