use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use aws_sdk_lambda::primitives::Blob;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS Lambda provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct LambdaConfig {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Default Lambda function name or ARN. Can be overridden per-action.
    pub function_name: Option<String>,

    /// Default function version or alias qualifier (e.g. `"$LATEST"`, `"prod"`).
    pub qualifier: Option<String>,
}

impl std::fmt::Debug for LambdaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LambdaConfig")
            .field("aws", &self.aws)
            .field("function_name", &self.function_name)
            .field("qualifier", &self.qualifier)
            .finish()
    }
}

impl LambdaConfig {
    /// Create a new `LambdaConfig` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            function_name: None,
            qualifier: None,
        }
    }

    /// Set the default function name or ARN.
    #[must_use]
    pub fn with_function_name(mut self, function_name: impl Into<String>) -> Self {
        self.function_name = Some(function_name.into());
        self
    }

    /// Set the default qualifier (version or alias).
    #[must_use]
    pub fn with_qualifier(mut self, qualifier: impl Into<String>) -> Self {
        self.qualifier = Some(qualifier.into());
        self
    }

    /// Set the endpoint URL override (for `LocalStack`).
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.aws.endpoint_url = Some(endpoint_url.into());
        self
    }

    /// Set the IAM role ARN to assume.
    #[must_use]
    pub fn with_role_arn(mut self, role_arn: impl Into<String>) -> Self {
        self.aws.role_arn = Some(role_arn.into());
        self
    }

    /// Set the STS session name for assume-role.
    #[must_use]
    pub fn with_session_name(mut self, session_name: impl Into<String>) -> Self {
        self.aws.session_name = Some(session_name.into());
        self
    }

    /// Set the external ID for cross-account trust policies.
    #[must_use]
    pub fn with_external_id(mut self, external_id: impl Into<String>) -> Self {
        self.aws.external_id = Some(external_id.into());
        self
    }
}

/// Invocation type for Lambda functions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InvocationType {
    /// Synchronous invocation (waits for response).
    #[default]
    RequestResponse,
    /// Asynchronous invocation (fire-and-forget).
    Event,
    /// Dry-run invocation (validates input without executing).
    DryRun,
}

impl InvocationType {
    fn as_sdk_type(&self) -> aws_sdk_lambda::types::InvocationType {
        match self {
            Self::RequestResponse => aws_sdk_lambda::types::InvocationType::RequestResponse,
            Self::Event => aws_sdk_lambda::types::InvocationType::Event,
            Self::DryRun => aws_sdk_lambda::types::InvocationType::DryRun,
        }
    }
}

/// Payload for the `invoke_function` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LambdaInvokePayload {
    /// Lambda function name or ARN. Overrides config default.
    pub function_name: Option<String>,

    /// Function version or alias qualifier. Overrides config default.
    pub qualifier: Option<String>,

    /// JSON payload to pass to the function.
    pub payload: Option<serde_json::Value>,

    /// Invocation type. Defaults to `request_response` (synchronous).
    #[serde(default)]
    pub invocation_type: InvocationType,
}

/// AWS Lambda provider for invoking Lambda functions.
pub struct LambdaProvider {
    config: LambdaConfig,
    client: aws_sdk_lambda::Client,
}

impl std::fmt::Debug for LambdaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LambdaProvider")
            .field("config", &self.config)
            .field("client", &"<LambdaClient>")
            .finish()
    }
}

impl LambdaProvider {
    /// Create a new `LambdaProvider` by building an AWS SDK client.
    pub async fn new(config: LambdaConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_lambda::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create a `LambdaProvider` with a pre-built client (for testing).
    pub fn with_client(config: LambdaConfig, client: aws_sdk_lambda::Client) -> Self {
        Self { config, client }
    }
}

impl Provider for LambdaProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-lambda"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-lambda"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Lambda payload");
        let payload: LambdaInvokePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let function_name = payload
            .function_name
            .as_deref()
            .or(self.config.function_name.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration(
                    "no function_name in payload or provider config".to_owned(),
                )
            })?;

        let qualifier = payload
            .qualifier
            .as_deref()
            .or(self.config.qualifier.as_deref());

        debug!(
            function_name = %function_name,
            invocation_type = ?payload.invocation_type,
            "invoking Lambda function"
        );

        let mut request = self
            .client
            .invoke()
            .function_name(function_name)
            .invocation_type(payload.invocation_type.as_sdk_type());

        if let Some(qualifier) = qualifier {
            request = request.qualifier(qualifier);
        }

        if let Some(ref invoke_payload) = payload.payload {
            let payload_bytes = serde_json::to_vec(invoke_payload)
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            request = request.payload(Blob::new(payload_bytes));
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "Lambda invoke failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let status_code = result.status_code();
        let function_error = result.function_error().map(String::from);

        // Parse the response payload if present.
        let response_payload = result
            .payload()
            .and_then(|blob| {
                let bytes = blob.as_ref();
                serde_json::from_slice::<serde_json::Value>(bytes).ok()
            })
            .unwrap_or(serde_json::Value::Null);

        if let Some(ref err) = function_error {
            error!(function_error = %err, "Lambda function returned an error");
            return Err(ProviderError::ExecutionFailed(format!(
                "Lambda function error: {err}"
            )));
        }

        info!(
            function_name = %function_name,
            status_code = status_code,
            "Lambda invocation succeeded"
        );

        Ok(ProviderResponse::success(serde_json::json!({
            "function_name": function_name,
            "status_code": status_code,
            "response": response_payload
        })))
    }

    #[instrument(skip(self), fields(provider = "aws-lambda"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Lambda health check");
        // List functions with a limit to verify connectivity.
        self.client
            .list_functions()
            .max_items(1)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Lambda health check failed");
                ProviderError::Connection(format!("Lambda health check failed: {e}"))
            })?;
        info!("Lambda health check passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = LambdaConfig::new("eu-central-1");
        assert_eq!(config.aws.region, "eu-central-1");
        assert!(config.function_name.is_none());
        assert!(config.qualifier.is_none());
    }

    #[test]
    fn config_builder_chain() {
        let config = LambdaConfig::new("us-west-2")
            .with_function_name("my-function")
            .with_qualifier("prod")
            .with_endpoint_url("http://localhost:4566");
        assert_eq!(config.function_name.as_deref(), Some("my-function"));
        assert_eq!(config.qualifier.as_deref(), Some("prod"));
        assert_eq!(
            config.aws.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
    }

    #[test]
    fn config_debug_format() {
        let config = LambdaConfig::new("us-east-1")
            .with_function_name("handler")
            .with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("LambdaConfig"));
        assert!(debug.contains("handler"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = LambdaConfig::new("us-east-1").with_function_name("my-fn");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: LambdaConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "us-east-1");
        assert_eq!(deserialized.function_name.as_deref(), Some("my-fn"));
    }

    #[test]
    fn deserialize_invoke_payload() {
        let json = serde_json::json!({
            "function_name": "my-handler",
            "payload": {"key": "value"},
            "invocation_type": "event"
        });
        let payload: LambdaInvokePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.function_name.as_deref(), Some("my-handler"));
        assert!(payload.payload.is_some());
        assert!(matches!(payload.invocation_type, InvocationType::Event));
    }

    #[test]
    fn deserialize_minimal_invoke_payload() {
        let json = serde_json::json!({});
        let payload: LambdaInvokePayload = serde_json::from_value(json).unwrap();
        assert!(payload.function_name.is_none());
        assert!(payload.payload.is_none());
        assert!(matches!(
            payload.invocation_type,
            InvocationType::RequestResponse
        ));
    }

    #[test]
    fn invocation_type_default_is_request_response() {
        let inv_type = InvocationType::default();
        assert!(matches!(inv_type, InvocationType::RequestResponse));
    }

    #[test]
    fn invocation_type_serde_roundtrip() {
        for variant in &["request_response", "event", "dry_run"] {
            let json = serde_json::json!(variant);
            let _: InvocationType = serde_json::from_value(json).unwrap();
        }
    }
}
