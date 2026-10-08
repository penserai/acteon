//! Authenticated individual-agent services. Receipts are host-owned provenance;
//! never derive their source context from model messages or task metadata.
use crate::{ActeonClient, Error, PermitReference, a2a::A2A_PROTOCOL_VERSION};
use acteon_core::{ExecutionContextReference, Task, TaskMessage};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

pub const AGENT_SOURCE_CONTEXT_HEADER: &str = "x-acteon-agent-source-context";
pub const AGENT_EXECUTION_CONTEXT_HEADER: &str = "x-acteon-execution-context";
const EXECUTION_PERMITS_HEADER: &str = "x-acteon-execution-permits";

/// Existing verified authority carried into a delegated peer invocation.
/// The server revalidates the opaque reference, caller credential and permits.
pub struct AgentServiceParent<'a> {
    pub execution_context: &'a ExecutionContextReference,
    pub permits: &'a [PermitReference],
}

/// Persist this receipt in host state. Acceptance is not proof of execution.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentServiceReceipt {
    pub task: Task,
    namespace: String,
    tenant: String,
    agent: String,
    task_id: String,
    source_context: String,
}
/// Restriction acknowledgement; task status continues to reflect provider evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentServiceStopReceipt {
    pub task: Task,
    pub future_starts_blocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_abort: Option<AgentServiceProviderAbort>,
}

/// Provider-side intervention remains separate from the durable start fence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentServiceProviderAbort {
    RestrictedOnly,
    Uncertain { attempt_id: String },
    Reconciled { proof_digest: String },
}

impl AgentServiceProviderAbort {
    fn valid(&self) -> bool {
        match self {
            Self::RestrictedOnly => true,
            Self::Uncertain { attempt_id } => valid_attempt_id(attempt_id),
            Self::Reconciled { proof_digest } => {
                proof_digest.len() == 64
                    && proof_digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }
        }
    }
}

fn valid_attempt_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].into_iter().all(|i| bytes[i] == b'-')
        && bytes[14] == b'5'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
        && bytes.iter().enumerate().all(|(i, b)| {
            [8, 13, 18, 23].contains(&i) || b.is_ascii_digit() || (b'a'..=b'f').contains(b)
        })
}

impl std::fmt::Debug for AgentServiceReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentServiceReceipt")
            .field("namespace", &self.namespace)
            .field("tenant", &self.tenant)
            .field("agent", &self.agent)
            .field("task_id", &self.task_id)
            .finish_non_exhaustive()
    }
}
impl AgentServiceReceipt {
    #[must_use]
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    #[must_use]
    pub fn source_context(&self) -> &str {
        &self.source_context
    }
}
fn segment(value: &str) -> Result<String, Error> {
    if matches!(value, "" | "." | "..") {
        return Err(Error::Configuration(
            "invalid agent service path segment".into(),
        ));
    }
    Ok(
        percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
            .to_string(),
    )
}
fn valid_source(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 8192
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
fn task_matches(task: &Task, namespace: &str, tenant: &str, id: Option<&str>) -> bool {
    task.namespace == namespace
        && task.tenant == tenant
        && !task.id.is_empty()
        && id.is_none_or(|id| task.id == id)
}
async fn response_task(response: reqwest::Response) -> Result<Task, Error> {
    if !response.status().is_success() {
        return Err(Error::Http {
            status: response.status().as_u16(),
            message: response
                .text()
                .await
                .map_err(|e| Error::Connection(e.to_string()))?,
        });
    }
    if response
        .headers()
        .get("a2a-version")
        .and_then(|v| v.to_str().ok())
        != Some(A2A_PROTOCOL_VERSION)
    {
        return Err(Error::Deserialization(
            "agent service response version missing or unsupported".into(),
        ));
    }
    response
        .json()
        .await
        .map_err(|e| Error::Deserialization(e.to_string()))
}
impl ActeonClient {
    /// Submit once with a stable message ID; retain the receipt separately from
    /// model data. On response loss, reuse that same message identity.
    pub async fn agent_service_send_message(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        message: &TaskMessage,
    ) -> Result<AgentServiceReceipt, Error> {
        self.agent_service_send_message_inner(namespace, tenant, agent, message, None)
            .await
    }

    /// Submit under an existing verified parent context. This only carries
    /// references; the server recovers authority and verifies current permits.
    pub async fn agent_service_send_message_with_parent(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        message: &TaskMessage,
        parent: AgentServiceParent<'_>,
    ) -> Result<AgentServiceReceipt, Error> {
        self.agent_service_send_message_inner(namespace, tenant, agent, message, Some(parent))
            .await
    }

    async fn agent_service_send_message_inner(
        &self,
        namespace: &str,
        tenant: &str,
        agent: &str,
        message: &TaskMessage,
        parent: Option<AgentServiceParent<'_>>,
    ) -> Result<AgentServiceReceipt, Error> {
        let path = format!(
            "/a2a/{}/{}/agents/{}/v1/message:send",
            segment(namespace)?,
            segment(tenant)?,
            segment(agent)?
        );
        let mut request = self
            .add_auth(self.client.post(format!("{}{path}", self.base_url)))
            .header("a2a-version", A2A_PROTOCOL_VERSION);
        if let Some(parent) = parent {
            if parent.permits.is_empty() {
                return Err(Error::Configuration(
                    "delegated agent service invocation requires permits".into(),
                ));
            }
            let context = serde_json::to_vec(parent.execution_context)
                .map_err(|e| Error::Configuration(e.to_string()))?;
            let permits = serde_json::to_string(parent.permits)
                .map_err(|e| Error::Configuration(e.to_string()))?;
            request = request
                .header(
                    AGENT_EXECUTION_CONTEXT_HEADER,
                    URL_SAFE_NO_PAD.encode(context),
                )
                .header(EXECUTION_PERMITS_HEADER, permits);
        }
        let response = request
            .json(&serde_json::json!({"message":message}))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        let source = response
            .headers()
            .get(AGENT_SOURCE_CONTEXT_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let task = response_task(response).await?;
        let source = source.filter(|s| valid_source(s)).ok_or_else(|| {
            Error::Deserialization(
                "agent service admission source context missing or malformed".into(),
            )
        })?;
        if !task_matches(&task, namespace, tenant, None) {
            return Err(Error::Deserialization(
                "agent service task scope mismatch".into(),
            ));
        }
        Ok(AgentServiceReceipt {
            task_id: task.id.clone(),
            task,
            namespace: namespace.into(),
            tenant: tenant.into(),
            agent: agent.into(),
            source_context: source,
        })
    }
    /// Stop future starts for the original accepted job. No automatic retry;
    /// response loss requires an explicit retry with the same retained receipt.
    pub async fn agent_service_stop_task(
        &self,
        receipt: &AgentServiceReceipt,
    ) -> Result<AgentServiceStopReceipt, Error> {
        if !valid_source(&receipt.source_context) {
            return Err(Error::Configuration(
                "invalid agent service source context".into(),
            ));
        }
        let path = format!(
            "/a2a/{}/{}/agents/{}/v1/tasks/{}/stop",
            segment(&receipt.namespace)?,
            segment(&receipt.tenant)?,
            segment(&receipt.agent)?,
            segment(&receipt.task_id)?
        );
        let response = self
            .add_auth(self.client.post(format!("{}{path}", self.base_url)))
            .header("a2a-version", A2A_PROTOCOL_VERSION)
            .header(AGENT_SOURCE_CONTEXT_HEADER, &receipt.source_context)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Http {
                status: response.status().as_u16(),
                message: response
                    .text()
                    .await
                    .map_err(|e| Error::Connection(e.to_string()))?,
            });
        }
        if response
            .headers()
            .get("a2a-version")
            .and_then(|v| v.to_str().ok())
            != Some(A2A_PROTOCOL_VERSION)
        {
            return Err(Error::Deserialization(
                "agent service response version missing or unsupported".into(),
            ));
        }
        let stopped: AgentServiceStopReceipt = response
            .json()
            .await
            .map_err(|e| Error::Deserialization(e.to_string()))?;
        if !stopped.future_starts_blocked
            || stopped
                .provider_abort
                .as_ref()
                .is_some_and(|status| !status.valid())
            || !task_matches(
                &stopped.task,
                &receipt.namespace,
                &receipt.tenant,
                Some(&receipt.task_id),
            )
        {
            return Err(Error::Deserialization(
                "agent service stop acknowledgement mismatch".into(),
            ));
        }
        Ok(stopped)
    }

    /// Observe exactly the retained job. This neither resumes nor invokes work.
    pub async fn agent_service_get_task(
        &self,
        receipt: &AgentServiceReceipt,
    ) -> Result<Task, Error> {
        if !valid_source(&receipt.source_context) {
            return Err(Error::Configuration(
                "invalid agent service source context".into(),
            ));
        }
        let path = format!(
            "/a2a/{}/{}/agents/{}/v1/tasks/{}",
            segment(&receipt.namespace)?,
            segment(&receipt.tenant)?,
            segment(&receipt.agent)?,
            segment(&receipt.task_id)?
        );
        let response = self
            .add_auth(self.client.get(format!("{}{path}", self.base_url)))
            .header("a2a-version", A2A_PROTOCOL_VERSION)
            .header(AGENT_SOURCE_CONTEXT_HEADER, &receipt.source_context)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        let task = response_task(response).await?;
        if !task_matches(
            &task,
            &receipt.namespace,
            &receipt.tenant,
            Some(&receipt.task_id),
        ) {
            return Err(Error::Deserialization(
                "agent service task identity mismatch".into(),
            ));
        }
        Ok(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, body::to_bytes, extract::Request, response::IntoResponse, routing::any,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, Ordering},
    };

    type RecordedCalls = Arc<Mutex<Vec<(String, Option<String>)>>>;

    struct Fixture {
        client: ActeonClient,
        calls: RecordedCalls,
        status: Arc<AtomicU16>,
        source: Arc<AtomicBool>,
        stop_payload: Arc<Mutex<Option<serde_json::Value>>>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    impl Fixture {
        async fn new() -> Self {
            let wire: serde_json::Value = serde_json::from_str(include_str!(
                "../../../clients/contract-fixtures/agent-services.json"
            ))
            .unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let status = Arc::new(AtomicU16::new(200));
            let source = Arc::new(AtomicBool::new(true));
            let stop_payload = Arc::new(Mutex::new(None));
            let (log, code, header, stopped) = (
                calls.clone(),
                status.clone(),
                source.clone(),
                stop_payload.clone(),
            );
            let app = Router::new().fallback(any(move |request: Request| {
                let (wire, log, code, header, stopped) = (
                    wire.clone(),
                    log.clone(),
                    code.clone(),
                    header.clone(),
                    stopped.clone(),
                );
                async move {
                    assert_eq!(request.headers()["authorization"], "Bearer caller-key");
                    assert_eq!(request.headers()["a2a-version"], "1.0");
                    let path = percent_encoding::percent_decode_str(request.uri().path())
                        .decode_utf8()
                        .unwrap()
                        .into_owned();
                    let source = request
                        .headers()
                        .get(AGENT_SOURCE_CONTEXT_HEADER)
                        .map(|h| h.to_str().unwrap().to_owned());
                    log.lock().unwrap().push((path.clone(), source.clone()));
                    let status =
                        axum::http::StatusCode::from_u16(code.load(Ordering::SeqCst)).unwrap();
                    if status != axum::http::StatusCode::OK {
                        return (
                            status,
                            [("location", "/redirected")],
                            Json(serde_json::json!({"error":"denied"})),
                        )
                            .into_response();
                    }
                    let raw = to_bytes(request.into_body(), 2 * 1024 * 1024)
                        .await
                        .unwrap();
                    let index = if raw.is_empty() {
                        usize::from(path.ends_with("job-2") || path.ends_with("job-2/stop"))
                    } else {
                        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
                        assert!(source.is_none());
                        usize::from(body["message"]["messageId"] == "m2")
                    };
                    let job = &wire["jobs"][index];
                    if let Some(source) = source {
                        assert_eq!(source, job["source_context"].as_str().unwrap());
                    }
                    let payload = if path.ends_with("/stop") {
                        assert!(raw.is_empty());
                        stopped
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or_else(|| job["stop_response"].clone())
                    } else {
                        job["task"].clone()
                    };
                    let mut response = Json(payload).into_response();
                    response
                        .headers_mut()
                        .insert("a2a-version", "1.0".parse().unwrap());
                    if header.load(Ordering::SeqCst) {
                        response.headers_mut().insert(
                            AGENT_SOURCE_CONTEXT_HEADER,
                            job["source_context"].as_str().unwrap().parse().unwrap(),
                        );
                    }
                    response
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let client = ActeonClient::builder(format!("http://{address}"))
                .api_key("caller-key")
                .build()
                .unwrap();
            Self {
                client,
                calls,
                status,
                source,
                stop_payload,
                server,
            }
        }
    }
    #[tokio::test]
    async fn original_receipt_identity_survives_task_mutation_and_host_serialization() {
        let fixture = Fixture::new().await;
        let first = TaskMessage::text("m1", acteon_core::TaskRole::User, "one");
        let second = TaskMessage::text("m2", acteon_core::TaskRole::User, "two");
        let (first, second) = tokio::join!(
            fixture
                .client
                .agent_service_send_message("prod", "acme", "notifier", &first),
            fixture
                .client
                .agent_service_send_message("prod", "acme", "notifier", &second)
        );
        let mut first = first.unwrap();
        first.task.id = "tampered-model-id".into();
        let restored: AgentServiceReceipt =
            serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
        let task = fixture
            .client
            .agent_service_get_task(&restored)
            .await
            .unwrap();
        assert_eq!(task.id, "job-1");
        assert_eq!(
            fixture
                .client
                .agent_service_get_task(&second.unwrap())
                .await
                .unwrap()
                .id,
            "job-2"
        );
        assert!(!format!("{restored:?}").contains(restored.source_context()));
        let stopped = fixture
            .client
            .agent_service_stop_task(&restored)
            .await
            .unwrap();
        assert!(stopped.future_starts_blocked);
        assert!(matches!(
            stopped.provider_abort,
            Some(AgentServiceProviderAbort::RestrictedOnly)
        ));
        assert_eq!(stopped.task.id, "job-1");
        assert_eq!(stopped.task.status.state, acteon_core::TaskState::Submitted);
        assert_eq!(fixture.calls.lock().unwrap().len(), 5);
    }
    #[tokio::test]
    async fn governed_parent_context_and_permits_are_request_local() {
        let wire: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/agent-services.json"
        ))
        .unwrap();
        let expected = wire.clone();
        let app = Router::new().fallback(any(move |request: Request| {
            let wire = expected.clone();
            async move {
                assert!(request.headers().get(AGENT_SOURCE_CONTEXT_HEADER).is_none());
                assert_eq!(
                    request.headers()[AGENT_EXECUTION_CONTEXT_HEADER],
                    wire["parent"]["execution_context"].as_str().unwrap()
                );
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(
                        request.headers()[EXECUTION_PERMITS_HEADER]
                            .to_str()
                            .unwrap()
                    )
                    .unwrap(),
                    wire["parent"]["permits"]
                );
                let mut response = Json(wire["jobs"][0]["task"].clone()).into_response();
                response
                    .headers_mut()
                    .insert("a2a-version", "1.0".parse().unwrap());
                response.headers_mut().insert(
                    AGENT_SOURCE_CONTEXT_HEADER,
                    wire["jobs"][0]["source_context"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                );
                response
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ActeonClient::builder(format!("http://{address}"))
            .build()
            .unwrap();
        let context: ExecutionContextReference = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(wire["parent"]["execution_context"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let permits: Vec<PermitReference> =
            serde_json::from_value(wire["parent"]["permits"].clone()).unwrap();
        client
            .agent_service_send_message_with_parent(
                "prod",
                "acme",
                "notifier",
                &TaskMessage::text("m1", acteon_core::TaskRole::User, "one"),
                AgentServiceParent {
                    execution_context: &context,
                    permits: &permits,
                },
            )
            .await
            .unwrap();
        server.abort();
    }
    #[test]
    fn provider_abort_status_validation_is_exact() {
        for status in [
            AgentServiceProviderAbort::RestrictedOnly,
            AgentServiceProviderAbort::Uncertain {
                attempt_id: "f47ac10b-58cc-5372-a567-0e02b2c3d479".into(),
            },
            AgentServiceProviderAbort::Reconciled {
                proof_digest: "a".repeat(64),
            },
        ] {
            assert!(status.valid());
        }
        for status in [
            AgentServiceProviderAbort::Uncertain {
                attempt_id: "f47ac10b-58cc-4372-a567-0e02b2c3d479".into(),
            },
            AgentServiceProviderAbort::Reconciled {
                proof_digest: "A".repeat(64),
            },
        ] {
            assert!(!status.valid());
        }
    }
    #[tokio::test]
    async fn missing_header_errors_and_redirects_never_return_fake_receipts_or_retry() {
        let fixture = Fixture::new().await;
        let message = TaskMessage::text("m1", acteon_core::TaskRole::User, "one");
        fixture.source.store(false, Ordering::SeqCst);
        assert!(matches!(
            fixture
                .client
                .agent_service_send_message("prod", "acme", "notifier", &message)
                .await,
            Err(Error::Deserialization(_))
        ));
        for status in [403, 404, 409, 429, 503, 307] {
            fixture.status.store(status, Ordering::SeqCst);
            assert!(
                matches!(fixture.client.agent_service_send_message("prod", "acme", "notifier", &message).await, Err(Error::Http { status: actual, .. }) if actual == status)
            );
        }
        assert_eq!(fixture.calls.lock().unwrap().len(), 7);
    }
    #[tokio::test]
    async fn stop_refuses_false_acknowledgements_foreign_tasks_and_failed_http_without_retry() {
        let f = Fixture::new().await;
        let receipt = f
            .client
            .agent_service_send_message(
                "prod",
                "acme",
                "notifier",
                &TaskMessage::text("m1", acteon_core::TaskRole::User, "one"),
            )
            .await
            .unwrap();
        let wire: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/agent-services.json"
        ))
        .unwrap();
        for payload in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!({"task":wire["jobs"][0]["task"],"future_starts_blocked":false}),
            serde_json::json!({"task":wire["jobs"][0]["task"],"future_starts_blocked":"true"}),
            serde_json::json!({"task":wire["jobs"][1]["task"],"future_starts_blocked":true}),
            serde_json::json!({"task":wire["jobs"][0]["task"],"future_starts_blocked":true,"provider_abort":{"state":"reconciled","proof_digest":"not-a-digest"}}),
        ] {
            *f.stop_payload.lock().unwrap() = Some(payload);
            assert!(matches!(
                f.client.agent_service_stop_task(&receipt).await,
                Err(Error::Deserialization(_))
            ));
        }
        for status in [403, 404, 409, 429, 503, 307] {
            f.status.store(status, Ordering::SeqCst);
            assert!(
                matches!(f.client.agent_service_stop_task(&receipt).await, Err(Error::Http {status:actual, ..}) if actual == status)
            );
        }
        assert_eq!(f.calls.lock().unwrap().len(), 13);
    }
}
