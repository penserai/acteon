//! Guarded HTTP verifier for host-bound `AuthRequired` challenges.

use acteon_core::PrincipalIdentity;
use acteon_crypto::{ExposeSecret, SecretString};
use acteon_gateway::{
    TaskAuthorizationVerification, TaskAuthorizationVerificationError, TaskAuthorizationVerifier,
    VerifiedTaskAuthorization,
};
use acteon_http::{GuardedClient, OutboundPolicy};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 16 * 1024;

pub struct TaskAuthorizationHttpVerifier {
    id: String,
    revision: u64,
    endpoint: String,
    credential: SecretString,
    client: GuardedClient,
}

impl TaskAuthorizationHttpVerifier {
    pub fn new_trusted_with_builder(
        id: String,
        revision: u64,
        endpoint: String,
        credential: SecretString,
        builder: reqwest::ClientBuilder,
        policy: OutboundPolicy,
        timeout: Duration,
    ) -> Result<Self, TaskAuthorizationVerificationError> {
        if id.is_empty()
            || revision == 0
            || credential.expose_secret().is_empty()
            || timeout.is_zero()
        {
            return Err(TaskAuthorizationVerificationError::Invalid(
                "invalid verifier installation".into(),
            ));
        }
        policy.validate_url(&endpoint).map_err(|_| {
            TaskAuthorizationVerificationError::Invalid("verifier endpoint refused".into())
        })?;
        let client =
            GuardedClient::from_builder(builder.timeout(timeout), policy, false).map_err(|_| {
                TaskAuthorizationVerificationError::Invalid(
                    "invalid guarded verifier client".into(),
                )
            })?;
        Ok(Self {
            id,
            revision,
            endpoint,
            credential,
            client,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerificationRequest<'a> {
    schema: u32,
    namespace: &'a str,
    tenant: &'a str,
    task_id: &'a str,
    challenge_id: &'a str,
    requirement_digest: &'a str,
    requirement: &'a acteon_core::TaskAuthorizationRequirement,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationResponse {
    schema: u32,
    task_id: String,
    challenge_id: String,
    authorization_request_digest: String,
    requirement_digest: String,
    decision_id: String,
    subject: PrincipalIdentity,
    verified_at: DateTime<Utc>,
    valid_until: DateTime<Utc>,
}

#[async_trait]
impl TaskAuthorizationVerifier for TaskAuthorizationHttpVerifier {
    fn verifier_id(&self) -> &str {
        &self.id
    }
    fn revision(&self) -> u64 {
        self.revision
    }

    async fn verify(
        &self,
        request: &TaskAuthorizationVerification,
    ) -> Result<VerifiedTaskAuthorization, TaskAuthorizationVerificationError> {
        if request.requirement.verifier_id != self.id
            || request.requirement.verifier_revision != self.revision
        {
            return Err(TaskAuthorizationVerificationError::Invalid(
                "verifier binding mismatch".into(),
            ));
        }
        let requirement_digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&request.requirement).map_err(|_| {
                TaskAuthorizationVerificationError::Invalid("invalid requirement".into())
            })?,
        ));
        let response = self
            .client
            .post(&self.endpoint)
            .map_err(|_| {
                TaskAuthorizationVerificationError::Invalid("verifier endpoint refused".into())
            })?
            .bearer_auth(self.credential.expose_secret())
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&VerificationRequest {
                schema: 1,
                namespace: &request.scope.namespace,
                tenant: &request.scope.tenant,
                task_id: &request.task_id,
                challenge_id: &request.challenge_id,
                requirement_digest: &requirement_digest,
                requirement: &request.requirement,
            })
            .send()
            .await
            .map_err(|_| TaskAuthorizationVerificationError::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            400 | 401 | 403 | 404 | 409 => return Err(TaskAuthorizationVerificationError::Denied),
            _ => return Err(TaskAuthorizationVerificationError::Unavailable),
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| !value.starts_with("application/json"))
        {
            return Err(TaskAuthorizationVerificationError::Invalid(
                "verifier response content type missing or unsupported".into(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(TaskAuthorizationVerificationError::Invalid(
                "verifier response too large".into(),
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| TaskAuthorizationVerificationError::Unavailable)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(TaskAuthorizationVerificationError::Invalid(
                    "verifier response too large".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let evidence: VerificationResponse = serde_json::from_slice(&bytes).map_err(|_| {
            TaskAuthorizationVerificationError::Invalid("malformed verifier response".into())
        })?;
        let expected_digest = hex::encode(Sha256::digest(
            request.requirement.authorization_request_id.as_bytes(),
        ));
        if evidence.schema != 1
            || evidence.task_id != request.task_id
            || evidence.challenge_id != request.challenge_id
            || evidence.authorization_request_digest != expected_digest
            || evidence.requirement_digest != requirement_digest
            || evidence.subject != request.requirement.recipient
            || evidence.decision_id.is_empty()
            || evidence.decision_id.len() > 512
            || evidence.decision_id.trim() != evidence.decision_id
            || evidence.decision_id.chars().any(char::is_control)
            || evidence.valid_until <= evidence.verified_at
        {
            return Err(TaskAuthorizationVerificationError::Invalid(
                "verifier response binding mismatch".into(),
            ));
        }
        Ok(VerifiedTaskAuthorization {
            decision_id: evidence.decision_id,
            subject: evidence.subject,
            verified_at: evidence.verified_at,
            valid_until: evidence.valid_until,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{PrincipalKind, TaskAuthorizationRequirement};
    use acteon_gateway::TaskScope;
    use axum::{Json, Router, http::StatusCode, routing::post};

    fn request() -> TaskAuthorizationVerification {
        TaskAuthorizationVerification {
            scope: TaskScope::new("city", "acme"),
            task_id: "task-1".into(),
            challenge_id: "challenge-1".into(),
            requirement: TaskAuthorizationRequirement {
                verifier_id: "workload".into(),
                verifier_revision: 3,
                authorization_request_id: "opaque-42".into(),
                recipient: PrincipalIdentity::new("agent/medic", PrincipalKind::Agent).unwrap(),
                credential_authority: "city-identity".into(),
                audience: "incident-api".into(),
                required_scopes: vec!["incident.resolve".into()],
            },
        }
    }

    async fn verifier(
        response_challenge: &'static str,
    ) -> (TaskAuthorizationHttpVerifier, tokio::task::JoinHandle<()>) {
        let app = Router::new().route("/verify", post(move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
            assert_eq!(headers[reqwest::header::AUTHORIZATION], "Bearer verifier-secret");
            assert_eq!(body["taskId"], "task-1");
            let now = Utc::now();
            (StatusCode::OK, Json(serde_json::json!({
                "schema": 1, "taskId": "task-1", "challengeId": response_challenge,
                "authorizationRequestDigest": hex::encode(Sha256::digest(b"opaque-42")),
                "requirementDigest": body["requirementDigest"],
                "decisionId": "decision-42", "subject": {"id":"agent/medic","kind":"agent"},
                "verifiedAt": now, "validUntil": now + chrono::Duration::minutes(5)
            })))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let verifier = TaskAuthorizationHttpVerifier::new_trusted_with_builder(
            "workload".into(),
            3,
            format!("http://{address}/verify"),
            SecretString::new("verifier-secret".into()),
            reqwest::Client::builder(),
            OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into()],
            },
            Duration::from_secs(2),
        )
        .unwrap();
        (verifier, server)
    }

    #[tokio::test]
    async fn accepts_only_exact_echoed_binding() {
        let (verifier, server) = verifier("challenge-1").await;
        let evidence = verifier.verify(&request()).await.unwrap();
        assert_eq!(evidence.decision_id, "decision-42");
        server.abort();
    }

    #[tokio::test]
    async fn rejects_success_for_another_challenge() {
        let (verifier, server) = verifier("challenge-other").await;
        assert!(matches!(verifier.verify(&request()).await,
            Err(TaskAuthorizationVerificationError::Invalid(message)) if message == "verifier response binding mismatch"));
        server.abort();
    }
}
