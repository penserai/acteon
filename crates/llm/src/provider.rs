//! Governed typed-JSON inference exposed as an Acteon provider.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde_json::Value;

use crate::{JsonResponseContract, TypedJsonModelClient, TypedModelError, VerifiedModelLock};

/// Configuration for a content-addressed JSON model provider.
#[derive(Clone)]
pub struct GovernedModelProviderConfig {
    /// Provider name used by Acteon actions.
    pub name: String,
    /// HTTP endpoint that accepts inference requests.
    pub endpoint: String,
    /// HTTP endpoint that reports `loaded` models and `revisions`.
    pub health_endpoint: String,
    /// Model governance lock file.
    pub lock_file: PathBuf,
    /// Root used to resolve contract paths in the lock.
    pub contracts_root: PathBuf,
    /// Locked contract containing the JSON Schema for responses.
    pub response_contract: String,
    /// Optional locked JSON value injected into every request.
    pub request_contract: Option<String>,
    /// Top-level request field receiving the locked request contract.
    pub request_contract_field: Option<String>,
    /// Optional top-level request field receiving the locked model name.
    pub model_field: Option<String>,
    /// Optional bearer credential.
    pub bearer_token: Option<String>,
    /// End-to-end request timeout.
    pub timeout: Duration,
    /// Maximum accepted response size after decompression.
    pub max_response_bytes: NonZeroUsize,
    /// Recheck served identity before every inference call.
    pub verify_identity_each_call: bool,
}

/// An Acteon provider that accepts JSON actions and returns governed JSON model output.
pub struct GovernedModelProvider {
    name: String,
    model: TypedJsonModelClient,
    health_client: reqwest::Client,
    health_endpoint: reqwest::Url,
    bearer_token: Option<String>,
    governance: VerifiedModelLock,
    response_contract_name: String,
    response_contract: JsonResponseContract<Value>,
    request_contract_name: Option<String>,
    request_contract: Option<Value>,
    request_contract_field: Option<String>,
    model_field: Option<String>,
    max_response_bytes: usize,
    verify_identity_each_call: bool,
}

#[derive(Deserialize)]
struct RuntimeIdentity {
    loaded: Vec<String>,
    revisions: BTreeMap<String, String>,
}

impl GovernedModelProvider {
    /// Load and verify local governance material and construct the provider.
    pub fn new(config: GovernedModelProviderConfig) -> Result<Self, ProviderError> {
        validate_config(&config)?;
        let governance = VerifiedModelLock::load(&config.lock_file)
            .map_err(|error| ProviderError::Configuration(error.to_string()))?;
        let contracts = governance
            .load_contracts(&config.contracts_root)
            .map_err(|error| ProviderError::Configuration(error.to_string()))?;
        let response_schema = contract_json(&contracts, &config.response_contract)?;
        let response_contract = JsonResponseContract::new(&response_schema)
            .map_err(|error| ProviderError::Configuration(error.to_string()))?;
        let request_contract = config
            .request_contract
            .as_deref()
            .map(|name| contract_json(&contracts, name))
            .transpose()?;

        let mut model = TypedJsonModelClient::new(&config.endpoint, config.timeout)
            .map_err(|error| ProviderError::Configuration(error.to_string()))?
            .with_max_response_bytes(config.max_response_bytes);
        if let Some(token) = &config.bearer_token {
            model = model.with_bearer_token(token.clone());
        }
        let health_endpoint = parse_endpoint(&config.health_endpoint)?;
        let health_client = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| ProviderError::Configuration(error.to_string()))?;

        Ok(Self {
            name: config.name,
            model,
            health_client,
            health_endpoint,
            bearer_token: config.bearer_token,
            governance,
            response_contract_name: config.response_contract,
            response_contract,
            request_contract_name: config.request_contract,
            request_contract,
            request_contract_field: config.request_contract_field,
            model_field: config.model_field,
            max_response_bytes: config.max_response_bytes.get(),
            verify_identity_each_call: config.verify_identity_each_call,
        })
    }

    /// Verify that the runtime serves exactly the locked model and revision.
    pub async fn verify_runtime_identity(&self) -> Result<(), ProviderError> {
        let mut request = self.health_client.get(self.health_endpoint.clone());
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| ProviderError::Connection(error.to_string()))?;
        if !response.status().is_success() {
            return Err(ProviderError::Connection(format!(
                "model health endpoint returned HTTP {}",
                response.status()
            )));
        }
        let body = read_limited(response, self.max_response_bytes).await?;
        let identity: RuntimeIdentity = serde_json::from_slice(&body)
            .map_err(|error| ProviderError::Serialization(error.to_string()))?;
        self.governance
            .verify_served_identity(&identity.loaded, &identity.revisions)
            .map_err(|error| ProviderError::ExecutionFailed(error.to_string()))
    }

    fn request(&self, payload: &Value) -> Result<Value, ProviderError> {
        if self.request_contract_field.is_none() && self.model_field.is_none() {
            return Ok(payload.clone());
        }
        let mut request = payload.as_object().cloned().ok_or_else(|| {
            ProviderError::ExecutionFailed(
                "governed model request payload must be a JSON object".to_owned(),
            )
        })?;
        if let (Some(field), Some(contract)) =
            (&self.request_contract_field, &self.request_contract)
        {
            request.insert(field.clone(), contract.clone());
        }
        if let Some(field) = &self.model_field {
            request.insert(
                field.clone(),
                Value::String(self.governance.policy.model.name.clone()),
            );
        }
        Ok(Value::Object(request))
    }

    fn response(&self, output: Value, elapsed: Duration) -> ProviderResponse {
        let model = &self.governance.policy.model;
        let mut response = ProviderResponse::success(output);
        response.headers.insert(
            "acteon-model-lock-digest".to_owned(),
            self.governance.lock_digest.clone(),
        );
        response.headers.insert(
            "acteon-model-repository".to_owned(),
            model.repository.clone(),
        );
        response
            .headers
            .insert("acteon-model-name".to_owned(), model.name.clone());
        response
            .headers
            .insert("acteon-model-revision".to_owned(), model.revision.clone());
        response.headers.insert(
            "acteon-model-response-contract".to_owned(),
            self.response_contract_name.clone(),
        );
        if let Some(contract) = &self.request_contract_name {
            response
                .headers
                .insert("acteon-model-request-contract".to_owned(), contract.clone());
        }
        response.headers.insert(
            "acteon-model-elapsed-ms".to_owned(),
            elapsed.as_millis().to_string(),
        );
        response
    }
}

impl Provider for GovernedModelProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        if self.verify_identity_each_call {
            self.verify_runtime_identity().await?;
        }
        let request = self.request(&action.payload)?;
        let response = self
            .model
            .invoke(&request, &self.response_contract)
            .await
            .map_err(map_model_error)?;
        Ok(self.response(response.output, response.elapsed))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        self.verify_runtime_identity().await
    }
}

fn validate_config(config: &GovernedModelProviderConfig) -> Result<(), ProviderError> {
    for (field, value) in [
        ("name", config.name.as_str()),
        ("response_contract", config.response_contract.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ProviderError::Configuration(format!(
                "governed model {field} cannot be empty"
            )));
        }
    }
    if config.request_contract.is_some() != config.request_contract_field.is_some() {
        return Err(ProviderError::Configuration(
            "request_contract and request_contract_field must be configured together".to_owned(),
        ));
    }
    if config.request_contract_field.is_some()
        && config.request_contract_field == config.model_field
    {
        return Err(ProviderError::Configuration(
            "request_contract_field and model_field must be different".to_owned(),
        ));
    }
    for (field, value) in [
        ("request_contract", config.request_contract.as_deref()),
        (
            "request_contract_field",
            config.request_contract_field.as_deref(),
        ),
        ("model_field", config.model_field.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(ProviderError::Configuration(format!(
                "governed model {field} cannot be empty"
            )));
        }
    }
    Ok(())
}

fn contract_json(
    contracts: &BTreeMap<String, Vec<u8>>,
    name: &str,
) -> Result<Value, ProviderError> {
    let bytes = contracts.get(name).ok_or_else(|| {
        ProviderError::Configuration(format!("model lock has no contract named '{name}'"))
    })?;
    serde_json::from_slice(bytes).map_err(|error| {
        ProviderError::Configuration(format!("model contract '{name}' is not JSON: {error}"))
    })
}

fn parse_endpoint(endpoint: &str) -> Result<reqwest::Url, ProviderError> {
    let endpoint = reqwest::Url::parse(endpoint)
        .map_err(|error| ProviderError::Configuration(error.to_string()))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ProviderError::Configuration(
            "model health endpoint must be an HTTP(S) URL without credentials or fragment"
                .to_owned(),
        ));
    }
    Ok(endpoint)
}

async fn read_limited(
    mut response: reqwest::Response,
    maximum: usize,
) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(ProviderError::Serialization(format!(
            "model health response exceeded the {maximum}-byte limit"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ProviderError::Connection(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(ProviderError::Serialization(format!(
                "model health response exceeded the {maximum}-byte limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn map_model_error(error: TypedModelError) -> ProviderError {
    match error {
        TypedModelError::Transport(message) => ProviderError::Connection(message),
        TypedModelError::HttpStatus { status, .. } if status.as_u16() == 429 => {
            ProviderError::RateLimited
        }
        TypedModelError::HttpStatus { status, body }
            if status.is_server_error() || matches!(status.as_u16(), 408 | 425) =>
        {
            ProviderError::Connection(format!("model runtime returned HTTP {status}: {body}"))
        }
        TypedModelError::HttpStatus { status, body } => {
            ProviderError::ExecutionFailed(format!("model runtime returned HTTP {status}: {body}"))
        }
        TypedModelError::Encode(message)
        | TypedModelError::Json(message)
        | TypedModelError::Decode(message) => ProviderError::Serialization(message),
        TypedModelError::ResponseTooLarge { limit } => {
            ProviderError::Serialization(format!("model response exceeded the {limit}-byte limit"))
        }
        TypedModelError::Schema {
            instance_path,
            schema_path,
        } => ProviderError::Serialization(format!(
            "model response violated its contract at {instance_path} (schema {schema_path})"
        )),
        TypedModelError::InvalidSchema(message)
        | TypedModelError::Client(message)
        | TypedModelError::InvalidEndpoint(message) => ProviderError::Configuration(message),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use acteon_provider::Provider;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;

    #[derive(Clone)]
    struct RuntimeState {
        requests: Arc<Mutex<Vec<Value>>>,
        revision: Arc<Mutex<String>>,
        valid_response: bool,
    }

    async fn health(
        State(state): State<RuntimeState>,
        headers: HeaderMap,
    ) -> Result<Json<Value>, StatusCode> {
        authorize(&headers)?;
        Ok(Json(json!({
            "loaded": ["detector"],
            "revisions": {"detector": state.revision.lock().unwrap().clone()}
        })))
    }

    async fn infer(
        State(state): State<RuntimeState>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> Result<Json<Value>, StatusCode> {
        authorize(&headers)?;
        state.requests.lock().unwrap().push(request);
        if state.valid_response {
            Ok(Json(json!({"label": "incident", "score": 0.91})))
        } else {
            Ok(Json(json!({"label": 7, "score": 0.91})))
        }
    }

    fn authorize(headers: &HeaderMap) -> Result<(), StatusCode> {
        match headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
        {
            Some("Bearer secret") => Ok(()),
            _ => Err(StatusCode::UNAUTHORIZED),
        }
    }

    async fn runtime(valid_response: bool) -> (String, RuntimeState) {
        let state = RuntimeState {
            requests: Arc::new(Mutex::new(Vec::new())),
            revision: Arc::new(Mutex::new("revision-1".to_owned())),
            valid_response,
        };
        let app = Router::new()
            .route("/health", get(health))
            .route("/infer", post(infer))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), state)
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    fn governed_config(root: &TempDir, base_url: &str) -> GovernedModelProviderConfig {
        let schema = serde_json::to_vec(&json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["label", "score"],
            "properties": {
                "label": {"type": "string"},
                "score": {"type": "number", "minimum": 0, "maximum": 1}
            }
        }))
        .unwrap();
        let questions = serde_json::to_vec(&json!({"incident": {"type": "noul"}})).unwrap();
        std::fs::write(root.path().join("response.json"), &schema).unwrap();
        std::fs::write(root.path().join("questions.json"), &questions).unwrap();
        let lock = serde_json::to_vec(&json!({
            "schema_version": 1,
            "runtime": {"engine": "1.0.0"},
            "model": {
                "repository": "example/detector",
                "name": "detector",
                "revision": "revision-1"
            },
            "artifacts": {"model.bin": format!("sha256:{}", "0".repeat(64))},
            "contracts": {
                "response": {"path": "response.json", "digest": sha256(&schema)},
                "questions": {"path": "questions.json", "digest": sha256(&questions)}
            }
        }))
        .unwrap();
        std::fs::write(root.path().join("model.lock.json"), lock).unwrap();
        GovernedModelProviderConfig {
            name: "model".to_owned(),
            endpoint: format!("{base_url}/infer"),
            health_endpoint: format!("{base_url}/health"),
            lock_file: root.path().join("model.lock.json"),
            contracts_root: root.path().to_owned(),
            response_contract: "response".to_owned(),
            request_contract: Some("questions".to_owned()),
            request_contract_field: Some("questions".to_owned()),
            model_field: Some("model".to_owned()),
            bearer_token: Some("secret".to_owned()),
            timeout: Duration::from_secs(2),
            max_response_bytes: NonZeroUsize::new(4096).unwrap(),
            verify_identity_each_call: true,
        }
    }

    #[tokio::test]
    async fn invokes_with_locked_fields_and_returns_governance_evidence() {
        let root = TempDir::new().unwrap();
        let (base_url, state) = runtime(true).await;
        let provider = GovernedModelProvider::new(governed_config(&root, &base_url)).unwrap();
        let action = Action::new(
            "observability",
            "acme",
            "model",
            "detect",
            json!({
                "state": {"latency_ms": 900},
                "questions": {"forged": true},
                "model": "unapproved"
            }),
        );

        let response = Provider::execute(&provider, &action).await.unwrap();

        assert_eq!(response.body["label"], "incident");
        assert_eq!(response.headers["acteon-model-name"], "detector");
        assert_eq!(response.headers["acteon-model-revision"], "revision-1");
        assert_eq!(
            response.headers["acteon-model-response-contract"],
            "response"
        );
        assert_eq!(
            response.headers["acteon-model-request-contract"],
            "questions"
        );
        assert!(response.headers["acteon-model-lock-digest"].starts_with("sha256:"));
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests[0]["model"], "detector");
        assert_eq!(
            requests[0]["questions"],
            json!({"incident": {"type": "noul"}})
        );
        assert_eq!(requests[0]["state"]["latency_ms"], 900);
    }

    #[tokio::test]
    async fn rejects_schema_invalid_output() {
        let root = TempDir::new().unwrap();
        let (base_url, _) = runtime(false).await;
        let provider = GovernedModelProvider::new(governed_config(&root, &base_url)).unwrap();
        let action = Action::new("ns", "tenant", "model", "detect", json!({}));

        let error = Provider::execute(&provider, &action).await.unwrap_err();

        assert!(matches!(error, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn rejects_runtime_revision_drift_before_inference() {
        let root = TempDir::new().unwrap();
        let (base_url, state) = runtime(true).await;
        let provider = GovernedModelProvider::new(governed_config(&root, &base_url)).unwrap();
        *state.revision.lock().unwrap() = "revision-2".to_owned();
        let action = Action::new("ns", "tenant", "model", "detect", json!({}));

        let error = Provider::execute(&provider, &action).await.unwrap_err();

        assert!(matches!(error, ProviderError::ExecutionFailed(_)));
        assert!(state.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_conflicting_injected_fields() {
        let root = TempDir::new().unwrap();
        let (base_url, _) = runtime(true).await;
        let mut config = governed_config(&root, &base_url);
        config.model_field = config.request_contract_field.clone();

        let error = GovernedModelProvider::new(config).err().unwrap();

        assert!(matches!(error, ProviderError::Configuration(_)));
    }

    #[test]
    fn preserves_transient_http_failure_semantics() {
        let limited = map_model_error(TypedModelError::HttpStatus {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            body: String::new(),
        });
        let timeout = map_model_error(TypedModelError::HttpStatus {
            status: reqwest::StatusCode::REQUEST_TIMEOUT,
            body: String::new(),
        });

        assert!(matches!(limited, ProviderError::RateLimited));
        assert!(matches!(timeout, ProviderError::Connection(_)));
        assert!(limited.is_retryable());
        assert!(timeout.is_retryable());
    }
}
