use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::PagerDutyConfig;
use crate::error::PagerDutyError;
use crate::types::{
    PagerDutyApiResponse, PagerDutyEvent, PagerDutyImage, PagerDutyLink, PagerDutyPayload,
};

/// `PagerDuty` provider that sends events via the `PagerDuty` Events API v2.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor.
pub struct PagerDutyProvider {
    config: PagerDutyConfig,
    client: Client,
}

/// Fields extracted from an action payload.
#[derive(Debug, Deserialize)]
struct EventPayload {
    event_action: String,
    summary: Option<String>,
    severity: Option<String>,
    source: Option<String>,
    component: Option<String>,
    group: Option<String>,
    class: Option<String>,
    dedup_key: Option<String>,
    custom_details: Option<serde_json::Value>,
    images: Option<Vec<PagerDutyImage>>,
    links: Option<Vec<PagerDutyLink>>,
}

impl PagerDutyProvider {
    /// Create a new `PagerDuty` provider with the given configuration.
    ///
    /// Uses a default `reqwest::Client` with reasonable timeouts.
    pub fn new(config: PagerDutyConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new `PagerDuty` provider with a custom HTTP client.
    ///
    /// Useful for testing or for sharing a connection pool across providers.
    pub fn with_client(config: PagerDutyConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Build the full URL for the Events API v2 enqueue endpoint.
    fn enqueue_url(&self) -> String {
        format!("{}/v2/enqueue", self.config.api_base_url)
    }

    /// Send an event to the `PagerDuty` Events API v2 and interpret the
    /// response.
    async fn send_event(
        &self,
        event: &PagerDutyEvent,
    ) -> Result<PagerDutyApiResponse, PagerDutyError> {
        let url = self.enqueue_url();

        debug!(event_action = %event.event_action, "sending event to PagerDuty");

        let response = self.client.post(&url).json(event).send().await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("PagerDuty API rate limit hit");
            return Err(PagerDutyError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PagerDutyError::Api(format!("HTTP {status}: {body}")));
        }

        let api_response: PagerDutyApiResponse = response.json().await?;

        Ok(api_response)
    }

    /// Build a [`PagerDutyEvent`] from the deserialized action payload,
    /// applying config defaults where appropriate.
    fn build_event(&self, payload: EventPayload) -> Result<PagerDutyEvent, PagerDutyError> {
        match payload.event_action.as_str() {
            "trigger" => {
                let summary = payload.summary.ok_or_else(|| {
                    PagerDutyError::InvalidPayload(
                        "trigger events require a 'summary' field".into(),
                    )
                })?;

                let severity = payload
                    .severity
                    .unwrap_or_else(|| self.config.default_severity.clone());

                let source = payload
                    .source
                    .or_else(|| self.config.default_source.clone())
                    .unwrap_or_else(|| "acteon".to_owned());

                Ok(PagerDutyEvent {
                    routing_key: self.config.routing_key.clone(),
                    event_action: "trigger".into(),
                    dedup_key: payload.dedup_key,
                    payload: Some(PagerDutyPayload {
                        summary,
                        source,
                        severity,
                        component: payload.component,
                        group: payload.group,
                        class: payload.class,
                        custom_details: payload.custom_details,
                    }),
                    images: payload.images,
                    links: payload.links,
                })
            }
            "acknowledge" | "resolve" => {
                let dedup_key = payload.dedup_key.ok_or_else(|| {
                    PagerDutyError::InvalidPayload(format!(
                        "{} events require a 'dedup_key' field",
                        payload.event_action,
                    ))
                })?;

                Ok(PagerDutyEvent {
                    routing_key: self.config.routing_key.clone(),
                    event_action: payload.event_action,
                    dedup_key: Some(dedup_key),
                    payload: None,
                    images: None,
                    links: None,
                })
            }
            other => Err(PagerDutyError::InvalidPayload(format!(
                "invalid event_action '{other}': must be 'trigger', 'acknowledge', or 'resolve'",
            ))),
        }
    }
}

impl Provider for PagerDutyProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "pagerduty"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "pagerduty"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: EventPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| PagerDutyError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let event = self.build_event(payload)?;
        let api_response = self.send_event(&event).await?;

        let body = serde_json::json!({
            "status": api_response.status,
            "message": api_response.message,
            "dedup_key": api_response.dedup_key,
        });

        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "pagerduty"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        let url = self.enqueue_url();

        debug!("performing PagerDuty health check");

        // POST an empty JSON body — any HTTP response (even 400) means the
        // endpoint is reachable. Only a connection failure is an error.
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;

        debug!(status = %response.status(), "PagerDuty health check response");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};

    use super::*;
    use crate::config::PagerDutyConfig;

    /// A minimal mock HTTP server built on tokio that returns canned responses.
    struct MockPagerDutyServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockPagerDutyServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind mock server");
            let port = listener.local_addr().unwrap().port();
            let base_url = format!("http://127.0.0.1:{port}");
            Self { listener, base_url }
        }

        /// Accept one connection and respond with the given status code and JSON
        /// body, then shut down.
        async fn respond_once(self, status_code: u16, body: &str) {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();

            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            // Read the full request (we don't parse it -- just drain it).
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await.unwrap();

            let response = format!(
                "HTTP/1.1 {status_code} OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }

        /// Accept one connection and respond with HTTP 429 (rate limited).
        async fn respond_rate_limited(self) {
            let body = r#"{"status":"throttle event creation","message":"Rate limit reached","dedup_key":null}"#;
            self.respond_once(429, body).await;
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("incidents", "tenant-1", "pagerduty", "send_event", payload)
    }

    #[test]
    fn provider_name() {
        let config = PagerDutyConfig::new("test-routing-key");
        let provider = PagerDutyProvider::new(config);
        assert_eq!(provider.name(), "pagerduty");
    }

    #[tokio::test]
    async fn execute_trigger_success() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "summary": "CPU usage exceeded 90%",
            "severity": "critical",
            "source": "monitoring",
            "dedup_key": "web-01/cpu-high"
        }));

        let response_body =
            r#"{"status":"success","message":"Event processed","dedup_key":"web-01/cpu-high"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(202, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["status"], "success");
        assert_eq!(response.body["dedup_key"], "web-01/cpu-high");
    }

    #[tokio::test]
    async fn execute_trigger_with_images_and_links() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "summary": "CPU usage high",
            "images": [
                {
                    "src": "https://example.com/chart.png",
                    "href": "https://example.com/dashboard",
                    "alt": "CPU Chart"
                }
            ],
            "links": [
                {
                    "href": "https://example.com/runbook",
                    "text": "Runbook"
                }
            ]
        }));

        let response_body = r#"{"status":"success","message":"Event processed","dedup_key":"abc"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(202, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_acknowledge_success() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "acknowledge",
            "dedup_key": "web-01/cpu-high"
        }));

        let response_body =
            r#"{"status":"success","message":"Event processed","dedup_key":"web-01/cpu-high"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(202, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["message"], "Event processed");
    }

    #[tokio::test]
    async fn execute_resolve_success() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "resolve",
            "dedup_key": "web-01/cpu-high"
        }));

        let response_body =
            r#"{"status":"success","message":"Event processed","dedup_key":"web-01/cpu-high"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(202, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
    }

    #[tokio::test]
    async fn execute_trigger_uses_config_defaults() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key")
            .with_api_base_url(&server.base_url)
            .with_default_severity("warning")
            .with_default_source("acteon-test");
        let provider = PagerDutyProvider::new(config);

        // Payload without severity or source — should use config defaults.
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "summary": "Disk space low"
        }));

        let response_body =
            r#"{"status":"success","message":"Event processed","dedup_key":"generated-key"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(202, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_trigger_missing_summary() {
        let config = PagerDutyConfig::new("test-key").with_api_base_url("http://localhost:1");
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "severity": "critical"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_invalid_event_action() {
        let config = PagerDutyConfig::new("test-key").with_api_base_url("http://localhost:1");
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "invalid_action",
            "summary": "test"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_ack_missing_dedup_key() {
        let config = PagerDutyConfig::new("test-key").with_api_base_url("http://localhost:1");
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "acknowledge"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "summary": "test event"
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_api_error() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "summary": "test event"
        }));

        let response_body = r#"{"status":"invalid event","message":"Event object is invalid"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(400, response_body).await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_success() {
        let server = MockPagerDutyServer::start().await;
        let config = PagerDutyConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = PagerDutyProvider::new(config);

        // Even a 400 response means the endpoint is reachable.
        let response_body = r#"{"status":"invalid event","message":"Event object is invalid"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(400, response_body).await;
        });

        let result = provider.health_check().await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_connection_failure() {
        // Point to a port that nothing is listening on.
        let config = PagerDutyConfig::new("test-key").with_api_base_url("http://127.0.0.1:1");
        let provider = PagerDutyProvider::new(config);

        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }
}
