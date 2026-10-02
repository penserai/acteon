//! Runtime-neutral typed JSON model invocation.
//!
//! The default client rejects redirects and ignores ambient proxy settings so
//! inference payloads and bearer credentials stay bound to the configured
//! endpoint. Responses are size-limited before JSON parsing or schema checks.

use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

/// Default maximum decompressed model response size: one mebibyte.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// A compiled JSON Schema paired with the Rust output type it protects.
pub struct JsonResponseContract<T> {
    validator: jsonschema::Validator,
    marker: PhantomData<fn() -> T>,
}

/// A successful typed inference with transport evidence.
#[derive(Debug)]
pub struct TypedModelResponse<T> {
    /// Validated and deserialized model output.
    pub output: T,
    /// Parsed JSON value used for schema validation.
    pub raw: Value,
    /// Exact bounded HTTP response body for audit evidence.
    pub raw_body: Vec<u8>,
    /// Response headers supplied by the model runtime.
    pub headers: HeaderMap,
    /// End-to-end client latency.
    pub elapsed: Duration,
}

/// HTTP client for JSON model runtimes.
pub struct TypedJsonModelClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    bearer_token: Option<String>,
    max_response_bytes: NonZeroUsize,
}

/// Errors raised before a model response can enter application policy.
#[derive(Debug, Error)]
pub enum TypedModelError {
    /// The response contract itself is not valid JSON Schema.
    #[error("invalid model response schema: {0}")]
    InvalidSchema(String),
    /// The HTTP client could not be constructed.
    #[error("failed to construct model HTTP client: {0}")]
    Client(String),
    /// The configured endpoint is not an HTTP(S) URL.
    #[error("invalid model endpoint: {0}")]
    InvalidEndpoint(String),
    /// The model request failed before a response was received.
    #[error("model request failed: {0}")]
    Transport(String),
    /// The typed request could not be encoded as JSON.
    #[error("model request could not encode as JSON: {0}")]
    Encode(String),
    /// The model runtime returned a non-success status.
    #[error("model runtime returned HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Bounded response text.
        body: String,
    },
    /// The response was not valid JSON.
    #[error("model response is not valid JSON: {0}")]
    Json(String),
    /// The decompressed response exceeded the configured limit.
    #[error("model response exceeded the {limit}-byte limit")]
    ResponseTooLarge {
        /// Configured response limit.
        limit: usize,
    },
    /// JSON Schema rejected the response.
    #[error("model response violated its schema at {instance_path} (schema {schema_path})")]
    Schema {
        /// JSON Pointer to the rejected value.
        instance_path: String,
        /// JSON Pointer to the violated schema keyword.
        schema_path: String,
    },
    /// The schema-valid response could not deserialize to the declared type.
    #[error("model response could not decode as the declared type: {0}")]
    Decode(String),
}

impl<T> JsonResponseContract<T>
where
    T: DeserializeOwned,
{
    /// Compile a response schema once for reuse across invocations.
    pub fn new(schema: &Value) -> Result<Self, TypedModelError> {
        let validator = jsonschema::validator_for(schema)
            .map_err(|error| TypedModelError::InvalidSchema(error.to_string()))?;
        Ok(Self {
            validator,
            marker: PhantomData,
        })
    }

    fn decode(&self, raw: Value) -> Result<T, TypedModelError> {
        if let Err(error) = self.validator.validate(&raw) {
            return Err(TypedModelError::Schema {
                instance_path: error.instance_path().to_string(),
                schema_path: error.schema_path().to_string(),
            });
        }
        serde_json::from_value(raw).map_err(|error| TypedModelError::Decode(error.to_string()))
    }
}

impl TypedJsonModelClient {
    /// Create a model client for one JSON endpoint.
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Result<Self, TypedModelError> {
        let endpoint = endpoint.into();
        let endpoint = reqwest::Url::parse(&endpoint)
            .map_err(|error| TypedModelError::InvalidEndpoint(error.to_string()))?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(TypedModelError::InvalidEndpoint(format!(
                "unsupported URL scheme {}",
                endpoint.scheme()
            )));
        }
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(TypedModelError::InvalidEndpoint(
                "embedded credentials and URL fragments are not allowed".to_owned(),
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| TypedModelError::Client(error.to_string()))?;
        Ok(Self {
            client,
            endpoint,
            bearer_token: None,
            max_response_bytes: NonZeroUsize::new(DEFAULT_MAX_RESPONSE_BYTES)
                .expect("the default model response limit is non-zero"),
        })
    }

    /// Attach bearer authentication to subsequent model calls.
    #[must_use]
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    /// Set the maximum decompressed response size accepted from the runtime.
    #[must_use]
    pub fn with_max_response_bytes(mut self, maximum: NonZeroUsize) -> Self {
        self.max_response_bytes = maximum;
        self
    }

    /// Invoke the model, validate its raw JSON, then deserialize the output.
    pub async fn invoke<I, O>(
        &self,
        input: &I,
        contract: &JsonResponseContract<O>,
    ) -> Result<TypedModelResponse<O>, TypedModelError>
    where
        I: Serialize + Sync,
        O: DeserializeOwned,
    {
        let started = Instant::now();
        let request_body = serde_json::to_vec(input)
            .map_err(|error| TypedModelError::Encode(error.to_string()))?;
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body);
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| TypedModelError::Transport(error.to_string()))?;
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            let body = read_prefix(response, 512).await.unwrap_or_default();
            return Err(TypedModelError::HttpStatus {
                status,
                body: sanitize_error_body(&body, 512),
            });
        }
        let body = read_limited(response, self.max_response_bytes.get()).await?;
        let raw = serde_json::from_slice::<Value>(&body)
            .map_err(|error| TypedModelError::Json(error.to_string()))?;
        let output = contract.decode(raw.clone())?;
        Ok(TypedModelResponse {
            output,
            raw,
            raw_body: body,
            headers,
            elapsed: started.elapsed(),
        })
    }
}

async fn read_limited(
    mut response: reqwest::Response,
    maximum: usize,
) -> Result<Vec<u8>, TypedModelError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(TypedModelError::ResponseTooLarge { limit: maximum });
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| TypedModelError::Transport(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(TypedModelError::ResponseTooLarge { limit: maximum });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_prefix(
    mut response: reqwest::Response,
    maximum: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut body = Vec::with_capacity(maximum);
    while body.len() < maximum {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        let remaining = maximum - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(body)
}

fn sanitize_error_body(value: &[u8], maximum: usize) -> String {
    let value = String::from_utf8_lossy(value);
    let mut output = String::with_capacity(value.len().min(maximum));
    'characters: for character in value.chars() {
        for escaped in character.escape_default() {
            if output.len() + escaped.len_utf8() > maximum {
                break 'characters;
            }
            output.push(escaped);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap as AxumHeaderMap, HeaderValue, StatusCode};
    use axum::response::Redirect;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use futures::stream;
    use serde::Deserialize;
    use serde::ser::{Error as _, Serializer};
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Verdict {
        label: String,
        score: f64,
    }

    struct Unencodable;

    impl Serialize for Unencodable {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("deliberate encoding failure"))
        }
    }

    fn contract() -> JsonResponseContract<Verdict> {
        JsonResponseContract::new(&json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["label", "score"],
            "properties": {
                "label": {"type": "string", "enum": ["healthy", "incident"]},
                "score": {"type": "number", "minimum": 0.0, "maximum": 1.0}
            }
        }))
        .unwrap()
    }

    #[test]
    fn client_and_contract_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TypedJsonModelClient>();
        assert_send_sync::<JsonResponseContract<Verdict>>();
    }

    #[test]
    fn validates_before_deserializing() {
        let parsed = contract()
            .decode(json!({"label": "incident", "score": 0.8}))
            .unwrap();
        assert_eq!(
            parsed,
            Verdict {
                label: "incident".to_owned(),
                score: 0.8
            }
        );
    }

    #[test]
    fn rejects_range_enum_and_unknown_field_violations() {
        for response in [
            json!({"label": "unknown", "score": 0.8}),
            json!({"label": "healthy", "score": 1.1}),
            json!({"label": "healthy", "score": 0.2, "extra": true}),
        ] {
            assert!(matches!(
                contract().decode(response),
                Err(TypedModelError::Schema { .. })
            ));
        }
    }

    #[test]
    fn rejects_non_http_endpoints() {
        assert!(matches!(
            TypedJsonModelClient::new("file:///tmp/model", Duration::from_secs(1)),
            Err(TypedModelError::InvalidEndpoint(_))
        ));
        assert!(matches!(
            TypedJsonModelClient::new(
                "https://user:password@example.com/model",
                Duration::from_secs(1)
            ),
            Err(TypedModelError::InvalidEndpoint(_))
        ));
    }

    #[test]
    fn schema_errors_do_not_echo_rejected_values() {
        let secret = "customer-secret-value";
        let error = contract()
            .decode(json!({"label": secret, "score": 0.2}))
            .unwrap_err();
        assert!(!error.to_string().contains(secret));

        let nested_contract = JsonResponseContract::<Value>::new(&json!({
            "oneOf": [
                {"type": "string", "const": "approved"},
                {"type": "integer"}
            ]
        }))
        .unwrap();
        let nested_error = nested_contract.decode(json!(secret)).unwrap_err();
        assert!(!nested_error.to_string().contains(secret));
    }

    #[test]
    fn sanitizes_error_bodies_for_safe_display() {
        let sanitized = sanitize_error_body(b"failure\n\x1b[31msecret", 512);
        assert_eq!(sanitized, "failure\\n\\u{1b}[31msecret");
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\u{1b}'));
    }

    async fn spawn_server(
        router: Router,
    ) -> (
        String,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });
        (format!("http://{address}"), shutdown_tx, task)
    }

    async fn stop_server(
        shutdown: tokio::sync::oneshot::Sender<()>,
        task: tokio::task::JoinHandle<()>,
    ) {
        let _ = shutdown.send(());
        task.await.unwrap();
    }

    fn connection_close() -> AxumHeaderMap {
        let mut headers = AxumHeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("close"));
        headers
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invokes_runtime_and_preserves_typed_and_raw_evidence() {
        async fn infer(
            headers: AxumHeaderMap,
            Json(_request): Json<Value>,
        ) -> (AxumHeaderMap, Json<Value>) {
            assert_eq!(
                headers["authorization"],
                HeaderValue::from_static("Bearer test-token")
            );
            let mut response_headers = AxumHeaderMap::new();
            response_headers.insert("x-inference-time-ms", HeaderValue::from_static("12.5"));
            response_headers.insert("connection", HeaderValue::from_static("close"));
            (
                response_headers,
                Json(json!({"label": "healthy", "score": 0.9})),
            )
        }

        let (base_url, shutdown, server) =
            spawn_server(Router::new().route("/infer", post(infer))).await;
        let client = TypedJsonModelClient::new(format!("{base_url}/infer"), Duration::from_secs(2))
            .unwrap()
            .with_bearer_token("test-token");
        let response = client
            .invoke(&json!({"signal": 1}), &contract())
            .await
            .unwrap();

        assert_eq!(response.output.label, "healthy");
        assert_eq!(response.raw["score"], 0.9);
        assert_eq!(
            serde_json::from_slice::<Value>(&response.raw_body).unwrap(),
            response.raw
        );
        assert_eq!(response.headers["x-inference-time-ms"], "12.5");
        stop_server(shutdown, server).await;
    }

    #[tokio::test]
    async fn reports_request_encoding_separately_from_transport() {
        let client =
            TypedJsonModelClient::new("http://127.0.0.1:1/infer", Duration::from_millis(10))
                .unwrap();
        let error = client.invoke(&Unencodable, &contract()).await.unwrap_err();
        assert!(matches!(error, TypedModelError::Encode(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rejects_oversized_success_response() {
        async fn infer() -> (AxumHeaderMap, Body) {
            let chunks = stream::iter([
                Ok::<_, std::convert::Infallible>(Bytes::from(vec![b'x'; 48])),
                Ok(Bytes::from(vec![b'x'; 48])),
            ]);
            (connection_close(), Body::from_stream(chunks))
        }

        let (base_url, shutdown, server) =
            spawn_server(Router::new().route("/infer", post(infer))).await;
        let client = TypedJsonModelClient::new(format!("{base_url}/infer"), Duration::from_secs(2))
            .unwrap()
            .with_max_response_bytes(NonZeroUsize::new(64).unwrap());
        let error = client.invoke(&json!({}), &contract()).await.unwrap_err();

        assert!(matches!(
            error,
            TypedModelError::ResponseTooLarge { limit: 64 }
        ));
        stop_server(shutdown, server).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bounds_non_success_response_bodies() {
        async fn fail() -> (StatusCode, AxumHeaderMap, String) {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                connection_close(),
                "x".repeat(4096),
            )
        }

        let (base_url, shutdown, server) =
            spawn_server(Router::new().route("/infer", post(fail))).await;
        let client =
            TypedJsonModelClient::new(format!("{base_url}/infer"), Duration::from_secs(2)).unwrap();
        let error = client.invoke(&json!({}), &contract()).await.unwrap_err();

        match error {
            TypedModelError::HttpStatus { status, body } => {
                assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(body.len(), 512);
            }
            other => panic!("expected bounded HTTP status error, got {other:?}"),
        }
        stop_server(shutdown, server).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_redirects_without_forwarding_the_request() {
        async fn redirect() -> (AxumHeaderMap, Redirect) {
            (connection_close(), Redirect::temporary("/sink"))
        }
        async fn sink(State(reached): State<Arc<AtomicBool>>) -> (StatusCode, AxumHeaderMap, ()) {
            reached.store(true, Ordering::SeqCst);
            (StatusCode::NO_CONTENT, connection_close(), ())
        }

        let reached = Arc::new(AtomicBool::new(false));
        let router = Router::new()
            .route("/infer", post(redirect))
            .route("/sink", get(sink).post(sink))
            .with_state(Arc::clone(&reached));
        let (base_url, shutdown, server) = spawn_server(router).await;
        let client =
            TypedJsonModelClient::new(format!("{base_url}/infer"), Duration::from_secs(2)).unwrap();
        let error = client
            .invoke(&json!({"private": "payload"}), &contract())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            TypedModelError::HttpStatus { status, .. } if status.is_redirection()
        ));
        assert!(!reached.load(Ordering::SeqCst));
        stop_server(shutdown, server).await;
    }
}
