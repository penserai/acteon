//! Runtime-neutral typed JSON model invocation.

use std::marker::PhantomData;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

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
    /// Unmodified JSON body for audit evidence.
    pub raw: Value,
    /// Response headers supplied by the model runtime.
    pub headers: HeaderMap,
    /// End-to-end client latency.
    pub elapsed: Duration,
}

/// HTTP client for JSON model runtimes.
pub struct TypedJsonModelClient {
    client: reqwest::Client,
    endpoint: String,
    bearer_token: Option<String>,
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
    /// The model request failed before a response was received.
    #[error("model request failed: {0}")]
    Transport(String),
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
    /// JSON Schema rejected the response.
    #[error("model response violated its schema: {0}")]
    Schema(String),
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
            return Err(TypedModelError::Schema(error.to_string()));
        }
        serde_json::from_value(raw).map_err(|error| TypedModelError::Decode(error.to_string()))
    }
}

impl TypedJsonModelClient {
    /// Create a model client for one JSON endpoint.
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Result<Self, TypedModelError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| TypedModelError::Client(error.to_string()))?;
        Ok(Self {
            client,
            endpoint: endpoint.into(),
            bearer_token: None,
        })
    }

    /// Attach bearer authentication to subsequent model calls.
    #[must_use]
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
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
        let mut request = self.client.post(&self.endpoint).json(input);
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
            let body = response.text().await.unwrap_or_default();
            return Err(TypedModelError::HttpStatus {
                status,
                body: bounded(&body, 512),
            });
        }
        let raw = response
            .json::<Value>()
            .await
            .map_err(|error| TypedModelError::Json(error.to_string()))?;
        let output = contract.decode(raw.clone())?;
        Ok(TypedModelResponse {
            output,
            raw,
            headers,
            elapsed: started.elapsed(),
        })
    }
}

fn bounded(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &value[..end])
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Verdict {
        label: String,
        score: f64,
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
                Err(TypedModelError::Schema(_))
            ));
        }
    }

    #[test]
    fn truncates_error_bodies_on_utf8_boundaries() {
        assert_eq!(bounded("ok", 8), "ok");
        assert_eq!(bounded("1234567é", 8), "1234567...[truncated]");
    }
}
