//! Guarded HTTP adapter for an exact operator-qualified Acteon A2A binding.

use acteon_core::{ExecutionContextReference, Task};
use acteon_crypto::{ExposeSecret, SecretString};
use acteon_executor::delegation::{
    PeerCancelDisposition, PeerSendDisposition, PeerSendRequest, PeerSubmissionCapability,
    PeerTaskObservation, PeerTaskRequest, PeerTransportAdapter, PeerTransportError,
};
use acteon_http::{GuardedClient, OutboundPolicy};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use std::time::Duration;

const MAX_TASK_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 2048;

/// Exact outbound adapter. The credential is host configuration and never
/// comes from a model, card, message, or public invocation body.
pub struct ActeonPeerHttpAdapter {
    binding_digest: String,
    revision: String,
    credential: SecretString,
    capability: PeerSubmissionCapability,
    client: GuardedClient,
}

impl ActeonPeerHttpAdapter {
    pub fn new_trusted(
        binding_digest: &str,
        revision: &str,
        credential: SecretString,
        capability: PeerSubmissionCapability,
        policy: OutboundPolicy,
        timeout: Duration,
    ) -> Result<Self, PeerTransportError> {
        Self::new_trusted_with_builder(
            binding_digest,
            revision,
            credential,
            capability,
            reqwest::Client::builder(),
            policy,
            timeout,
        )
    }

    pub fn new_trusted_with_builder(
        binding_digest: &str,
        revision: &str,
        credential: SecretString,
        capability: PeerSubmissionCapability,
        builder: reqwest::ClientBuilder,
        policy: OutboundPolicy,
        timeout: Duration,
    ) -> Result<Self, PeerTransportError> {
        if !valid_digest(binding_digest)
            || !valid_text(revision)
            || credential.expose_secret().is_empty()
            || timeout.is_zero()
        {
            return Err(PeerTransportError::Invalid);
        }
        let client = GuardedClient::from_builder(builder.timeout(timeout), policy, false)
            .map_err(|_| PeerTransportError::Invalid)?;
        Ok(Self {
            binding_digest: binding_digest.into(),
            revision: revision.into(),
            credential,
            capability,
            client,
        })
    }
}

#[async_trait]
impl PeerTransportAdapter for ActeonPeerHttpAdapter {
    fn revision(&self) -> &str {
        &self.revision
    }

    fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    fn submission_capability(&self) -> PeerSubmissionCapability {
        self.capability
    }

    async fn send(
        &self,
        request: PeerSendRequest<'_>,
    ) -> Result<PeerSendDisposition, PeerTransportError> {
        if request.transport != "rest" {
            return Ok(PeerSendDisposition::Rejected {
                code: "unsupported_peer_transport".into(),
            });
        }
        let context =
            serde_json::to_vec(request.parent).map_err(|_| PeerTransportError::Invalid)?;
        let permits =
            serde_json::to_string(request.permits).map_err(|_| PeerTransportError::Invalid)?;
        let Ok(builder) = self.client.post(request.endpoint) else {
            return Ok(PeerSendDisposition::Rejected {
                code: "peer_destination_refused".into(),
            });
        };
        let Ok(response) = builder
            .bearer_auth(self.credential.expose_secret())
            .header("a2a-version", "1.0")
            .header(
                "x-acteon-execution-context",
                URL_SAFE_NO_PAD.encode(context),
            )
            .header("x-acteon-execution-permits", permits)
            .json(&serde_json::json!({"message":request.message}))
            .send()
            .await
        else {
            return Ok(PeerSendDisposition::Uncertain);
        };
        let status = response.status();
        if status.is_success() {
            return accepted(response).await;
        }
        if matches!(status.as_u16(), 400 | 401 | 403 | 404 | 409 | 429) {
            let code = rejection_code(response, status.as_u16()).await;
            return Ok(PeerSendDisposition::Rejected { code });
        }
        Ok(PeerSendDisposition::Uncertain)
    }

    async fn observe_task(
        &self,
        request: PeerTaskRequest<'_>,
    ) -> Result<PeerTaskObservation, PeerTransportError> {
        if request.transport != "rest" {
            return Err(PeerTransportError::Refused);
        }
        let endpoint = task_endpoint(request.endpoint, request.task_id)?;
        let context =
            serde_json::to_vec(request.source_context).map_err(|_| PeerTransportError::Invalid)?;
        let mut builder = self
            .client
            .request(reqwest::Method::GET, &endpoint)
            .map_err(|_| PeerTransportError::Refused)?
            .bearer_auth(self.credential.expose_secret())
            .header("a2a-version", "1.0")
            .header(
                "x-acteon-agent-source-context",
                URL_SAFE_NO_PAD.encode(context),
            );
        if let Some(cursor) = request.progress_cursor {
            if !valid_cursor(cursor) {
                return Err(PeerTransportError::Invalid);
            }
            builder = builder.header(reqwest::header::IF_NONE_MATCH, cursor);
        }
        let response = builder
            .send()
            .await
            .map_err(|_| PeerTransportError::Unavailable)?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            if response
                .headers()
                .get("a2a-version")
                .and_then(|value| value.to_str().ok())
                != Some("1.0")
            {
                return Err(PeerTransportError::Unavailable);
            }
            let cursor = response_cursor(&response)?.ok_or(PeerTransportError::Unavailable)?;
            if request.progress_cursor != Some(cursor.as_str()) {
                return Err(PeerTransportError::Unavailable);
            }
            return Ok(PeerTaskObservation::Unchanged {
                progress_cursor: cursor,
            });
        }
        if !response.status().is_success() {
            return Err(
                if matches!(
                    response.status().as_u16(),
                    400 | 401 | 403 | 404 | 409 | 429
                ) {
                    PeerTransportError::Refused
                } else {
                    PeerTransportError::Unavailable
                },
            );
        }
        if response
            .headers()
            .get("a2a-version")
            .and_then(|value| value.to_str().ok())
            != Some("1.0")
        {
            return Err(PeerTransportError::Unavailable);
        }
        let progress_cursor = response_cursor(&response)?;
        if request.progress_cursor.is_some() && progress_cursor.is_none() {
            return Err(PeerTransportError::Unavailable);
        }
        let raw = read_bounded(response, MAX_TASK_BYTES).await?;
        let task = serde_json::from_slice(&raw).map_err(|_| PeerTransportError::Unavailable)?;
        Ok(PeerTaskObservation::Updated {
            task: Box::new(task),
            progress_cursor,
        })
    }

    async fn cancel_task(
        &self,
        request: PeerTaskRequest<'_>,
    ) -> Result<PeerCancelDisposition, PeerTransportError> {
        if request.transport != "rest" {
            return Ok(PeerCancelDisposition::Unsupported);
        }
        let endpoint = stop_endpoint(request.endpoint, request.task_id)?;
        let context =
            serde_json::to_vec(request.source_context).map_err(|_| PeerTransportError::Invalid)?;
        let Ok(builder) = self.client.request(reqwest::Method::POST, &endpoint) else {
            return Ok(PeerCancelDisposition::Rejected {
                code: "peer_destination_refused".into(),
            });
        };
        let Ok(response) = builder
            .bearer_auth(self.credential.expose_secret())
            .header("a2a-version", "1.0")
            .header(
                "x-acteon-agent-source-context",
                URL_SAFE_NO_PAD.encode(context),
            )
            .send()
            .await
        else {
            return Ok(PeerCancelDisposition::Uncertain);
        };
        let status = response.status();
        if status.is_success() {
            if response
                .headers()
                .get("a2a-version")
                .and_then(|value| value.to_str().ok())
                != Some("1.0")
            {
                return Ok(PeerCancelDisposition::Uncertain);
            }
            return Ok(match read_bounded(response, MAX_TASK_BYTES).await {
                Ok(raw) => serde_json::from_slice::<PeerStopResponse>(&raw).map_or(
                    PeerCancelDisposition::Uncertain,
                    |receipt| {
                        if !receipt.future_starts_blocked {
                            PeerCancelDisposition::Uncertain
                        } else if receipt.task.status.state.is_terminal() {
                            PeerCancelDisposition::Final {
                                task: Box::new(receipt.task),
                            }
                        } else {
                            PeerCancelDisposition::Restricted {
                                task: Box::new(receipt.task),
                            }
                        }
                    },
                ),
                Err(_) => PeerCancelDisposition::Uncertain,
            });
        }
        if matches!(status.as_u16(), 405 | 501) {
            return Ok(PeerCancelDisposition::Unsupported);
        }
        if matches!(status.as_u16(), 400 | 401 | 403 | 404 | 429) {
            return Ok(PeerCancelDisposition::Rejected {
                code: rejection_code(response, status.as_u16()).await,
            });
        }
        Ok(PeerCancelDisposition::Uncertain)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerStopResponse {
    task: Task,
    future_starts_blocked: bool,
    #[serde(default, rename = "provider_abort")]
    _provider_abort: Option<serde_json::Value>,
}

fn task_endpoint(endpoint: &str, task_id: &str) -> Result<String, PeerTransportError> {
    if task_id.is_empty()
        || task_id.len() > 1024
        || matches!(task_id, "." | "..")
        || task_id.chars().any(char::is_control)
    {
        return Err(PeerTransportError::Invalid);
    }
    let mut url = reqwest::Url::parse(endpoint).map_err(|_| PeerTransportError::Invalid)?;
    if url.query().is_some()
        || url.fragment().is_some()
        || url.path_segments().and_then(Iterator::last) != Some("message:send")
    {
        return Err(PeerTransportError::Refused);
    }
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| PeerTransportError::Refused)?;
        segments.pop().push("tasks").push(task_id);
    }
    Ok(url.into())
}

fn stop_endpoint(endpoint: &str, task_id: &str) -> Result<String, PeerTransportError> {
    Ok(format!("{}/stop", task_endpoint(endpoint, task_id)?))
}

async fn accepted(response: reqwest::Response) -> Result<PeerSendDisposition, PeerTransportError> {
    if response
        .headers()
        .get("a2a-version")
        .and_then(|v| v.to_str().ok())
        != Some("1.0")
    {
        return Ok(PeerSendDisposition::Uncertain);
    }
    let sources: Vec<_> = response
        .headers()
        .get_all("x-acteon-agent-source-context")
        .iter()
        .collect();
    if sources.len() != 1 || sources[0].as_bytes().len() > 8192 {
        return Ok(PeerSendDisposition::Uncertain);
    }
    let source_context = sources[0]
        .to_str()
        .ok()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .and_then(|raw| serde_json::from_slice::<ExecutionContextReference>(&raw).ok());
    let Ok(progress_cursor) = response_cursor(&response) else {
        return Ok(PeerSendDisposition::Uncertain);
    };
    let body = read_bounded(response, MAX_TASK_BYTES).await;
    let task = body
        .ok()
        .and_then(|raw| serde_json::from_slice::<Task>(&raw).ok());
    match (task, source_context) {
        (Some(task), Some(source_context)) => Ok(PeerSendDisposition::Accepted {
            task: Box::new(task),
            source_context,
            progress_cursor,
        }),
        _ => Ok(PeerSendDisposition::Uncertain),
    }
}

fn response_cursor(response: &reqwest::Response) -> Result<Option<String>, PeerTransportError> {
    let values: Vec<_> = response
        .headers()
        .get_all(reqwest::header::ETAG)
        .iter()
        .collect();
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() != 1 {
        return Err(PeerTransportError::Unavailable);
    }
    values[0]
        .to_str()
        .ok()
        .filter(|value| valid_cursor(value))
        .map(str::to_owned)
        .map(Some)
        .ok_or(PeerTransportError::Unavailable)
}

fn valid_cursor(value: &str) -> bool {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    !inner.is_empty()
        && value.len() <= 512
        && inner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

async fn rejection_code(response: reqwest::Response, status: u16) -> String {
    read_bounded(response, MAX_ERROR_BYTES)
        .await
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("error")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .filter(|code| valid_text(code))
        .unwrap_or_else(|| format!("peer_http_{status}"))
}

async fn read_bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, PeerTransportError> {
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Err(PeerTransportError::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| PeerTransportError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(PeerTransportError::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
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
    use acteon_core::{PrincipalIdentity, PrincipalKind, TaskMessage, TaskRole};
    use acteon_governance::permit::PermitReference;
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{get, post},
    };
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    fn parent() -> ExecutionContextReference {
        ExecutionContextReference::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "city".into(),
            "tenant".into(),
            PrincipalIdentity::new("caller-agent", PrincipalKind::Agent).unwrap(),
            "a".repeat(64),
        )
        .unwrap()
    }

    fn adapter() -> ActeonPeerHttpAdapter {
        ActeonPeerHttpAdapter::new_trusted(
            &"a".repeat(64),
            "acteon-a2a-v1",
            SecretString::new("caller-secret".into()),
            PeerSubmissionCapability::VerifiedIdempotent,
            OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into()],
            },
            Duration::from_secs(1),
        )
        .unwrap()
    }

    fn assert_accepted_cursor(disposition: PeerSendDisposition, expected: &str) {
        assert!(matches!(
            disposition,
            PeerSendDisposition::Accepted { progress_cursor: Some(cursor), .. }
                if cursor == expected
        ));
    }

    #[test]
    fn remote_task_endpoint_stays_under_the_qualified_agent_service() {
        assert_eq!(
            task_endpoint(
                "https://peer.example/a2a/city/tenant/agents/responder/v1/message:send",
                "task/one",
            )
            .unwrap(),
            "https://peer.example/a2a/city/tenant/agents/responder/v1/tasks/task%2Fone"
        );
        for endpoint in [
            "https://peer.example/a2a/city/tenant/v1/tasks/other",
            "https://peer.example/a2a/city/tenant/v1/message:send?redirect=other",
        ] {
            assert!(task_endpoint(endpoint, "task").is_err());
        }
        assert!(task_endpoint("https://peer.example/v1/message:send", "..").is_err());
        assert_eq!(
            stop_endpoint("https://peer.example/v1/message:send", "task/one").unwrap(),
            "https://peer.example/v1/tasks/task%2Fone/stop"
        );
    }

    #[tokio::test]
    async fn guarded_adapter_sends_exact_host_context_and_classifies_responses() {
        let parent = parent();
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parent).unwrap());
        let seen = Arc::new(Mutex::new(false));
        let captured = seen.clone();
        let source = encoded.clone();
        let bad_cursor_source = encoded.clone();
        let app = Router::new()
            .route(
                "/ok",
                post(
                    move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                        let captured = captured.clone();
                        let source = source.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer caller-secret");
                            assert_eq!(headers["a2a-version"], "1.0");
                            assert_eq!(headers["x-acteon-execution-context"], source);
                            assert_eq!(
                                serde_json::from_str::<serde_json::Value>(
                                    headers["x-acteon-execution-permits"].to_str().unwrap()
                                )
                                .unwrap(),
                                serde_json::json!([{"id":"caller-peer","accepted_revision":1}])
                            );
                            assert_eq!(body["message"]["messageId"], "peer-message");
                            *captured.lock().unwrap() = true;
                            (
                                StatusCode::OK,
                                [
                                    ("a2a-version", "1.0"),
                                    ("x-acteon-agent-source-context", source.as_str()),
                                    ("etag", "\"cursor-1\""),
                                ],
                                Json(Task::new("remote-task", "city", "tenant")),
                            )
                                .into_response()
                        }
                    },
                ),
            )
            .route(
                "/bad-cursor",
                post(move || {
                    let source = bad_cursor_source.clone();
                    async move {
                        (
                            StatusCode::OK,
                            [
                                ("a2a-version", "1.0".to_string()),
                                ("x-acteon-agent-source-context", source),
                                ("etag", "W/\"cursor-1\"".to_string()),
                            ],
                            Json(Task::new("remote-task", "city", "tenant")),
                        )
                    }
                }),
            )
            .route(
                "/deny",
                post(|| async {
                    (
                        StatusCode::FORBIDDEN,
                        Json(serde_json::json!({"error":"peer_denied"})),
                    )
                }),
            )
            .route(
                "/unavailable",
                post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            )
            .route(
                "/redirect",
                post(|| async { (StatusCode::TEMPORARY_REDIRECT, [("location", "/ok")]) }),
            )
            .route(
                "/malformed",
                post(|| async { Json(Task::new("remote-task", "city", "tenant")) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let permits = [PermitReference {
            id: "caller-peer".into(),
            accepted_revision: 1,
        }];
        let message = TaskMessage::text("peer-message", TaskRole::User, "diagnose");
        let adapter = adapter();
        let send = |path: &str| {
            let adapter = &adapter;
            let endpoint = format!("http://{address}/{path}");
            let parent = &parent;
            let permits = &permits;
            let message = &message;
            async move {
                adapter
                    .send(PeerSendRequest {
                        endpoint: &endpoint,
                        transport: "rest",
                        parent,
                        permits,
                        message,
                    })
                    .await
                    .unwrap()
            }
        };
        assert_accepted_cursor(send("ok").await, "\"cursor-1\"");
        assert!(*seen.lock().unwrap());
        assert!(matches!(
            send("deny").await,
            PeerSendDisposition::Rejected { code } if code == "peer_denied"
        ));
        for path in ["unavailable", "redirect", "malformed", "bad-cursor"] {
            assert!(matches!(send(path).await, PeerSendDisposition::Uncertain));
        }
        server.abort();
    }

    #[tokio::test]
    async fn guarded_observation_uses_and_validates_the_opaque_task_cursor() {
        let parent = parent();
        let source = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parent).unwrap());
        let app = Router::new().route(
            "/a2a/city/tenant/agents/responder/v1/tasks/remote-task",
            get(move |headers: HeaderMap| {
                let source = source.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer caller-secret");
                    assert_eq!(headers["a2a-version"], "1.0");
                    assert_eq!(headers["x-acteon-agent-source-context"], source);
                    if headers
                        .get(reqwest::header::IF_NONE_MATCH)
                        .is_some_and(|value| value == "\"cursor-1\"")
                    {
                        return (
                            StatusCode::NOT_MODIFIED,
                            [("a2a-version", "1.0"), ("etag", "\"cursor-1\"")],
                        )
                            .into_response();
                    }
                    (
                        StatusCode::OK,
                        [("a2a-version", "1.0"), ("etag", "\"cursor-1\"")],
                        Json(Task::new("remote-task", "city", "tenant")),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let endpoint = format!("http://{address}/a2a/city/tenant/agents/responder/v1/message:send");
        let adapter = adapter();
        let first = adapter
            .observe_task(PeerTaskRequest {
                endpoint: &endpoint,
                transport: "rest",
                source_context: &parent,
                task_id: "remote-task",
                progress_cursor: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            first,
            PeerTaskObservation::Updated { progress_cursor: Some(cursor), .. }
                if cursor == "\"cursor-1\""
        ));
        let unchanged = adapter
            .observe_task(PeerTaskRequest {
                endpoint: &endpoint,
                transport: "rest",
                source_context: &parent,
                task_id: "remote-task",
                progress_cursor: Some("\"cursor-1\""),
            })
            .await
            .unwrap();
        assert!(matches!(
            unchanged,
            PeerTaskObservation::Unchanged { progress_cursor }
                if progress_cursor == "\"cursor-1\""
        ));
        server.abort();
    }

    #[tokio::test]
    async fn guarded_adapter_uses_native_stop_and_preserves_restriction_only_truth() {
        let parent = parent();
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parent).unwrap());
        let source = encoded.clone();
        let app = Router::new().route(
            "/a2a/city/tenant/agents/responder/v1/tasks/remote-task/stop",
            post(move |headers: HeaderMap| {
                let source = source.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer caller-secret");
                    assert_eq!(headers["a2a-version"], "1.0");
                    assert_eq!(headers["x-acteon-agent-source-context"], source);
                    (
                        StatusCode::OK,
                        [("a2a-version", "1.0")],
                        Json(serde_json::json!({
                            "task": Task::new("remote-task", "city", "tenant"),
                            "future_starts_blocked": true,
                            "provider_abort": {"state": "restricted_only"}
                        })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let endpoint = format!("http://{address}/a2a/city/tenant/agents/responder/v1/message:send");
        let disposition = adapter()
            .cancel_task(PeerTaskRequest {
                endpoint: &endpoint,
                transport: "rest",
                task_id: "remote-task",
                source_context: &parent,
                progress_cursor: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            disposition,
            PeerCancelDisposition::Restricted { task }
                if task.id == "remote-task" && task.status.state == acteon_core::TaskState::Submitted
        ));
        server.abort();
    }

    #[tokio::test]
    async fn destination_policy_refuses_before_network_delivery() {
        let adapter = ActeonPeerHttpAdapter::new_trusted(
            &"a".repeat(64),
            "acteon-a2a-v1",
            SecretString::new("secret".into()),
            PeerSubmissionCapability::AtMostOnce,
            OutboundPolicy::default(),
            Duration::from_millis(10),
        )
        .unwrap();
        let parent = parent();
        let permits = [PermitReference {
            id: "p".into(),
            accepted_revision: 1,
        }];
        let message = TaskMessage::text("peer-message", TaskRole::User, "diagnose");
        assert!(matches!(
            adapter
                .send(PeerSendRequest {
                    endpoint: "http://127.0.0.1:1/a2a",
                    transport: "rest",
                    parent: &parent,
                    permits: &permits,
                    message: &message,
                })
                .await
                .unwrap(),
            PeerSendDisposition::Rejected { code } if code == "peer_destination_refused"
        ));
    }
}
