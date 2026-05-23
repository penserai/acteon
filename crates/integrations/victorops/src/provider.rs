use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError, truncate_error_body};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::VictorOpsConfig;
use crate::error::VictorOpsError;
use crate::types::{VictorOpsAlertRequest, VictorOpsApiResponse, VictorOpsMessageType};

/// Characters that must be percent-encoded inside an URL path
/// segment. Start from the IETF `CONTROLS` set and add every
/// sub-delim, query-start, and path-separator byte that would be
/// misinterpreted by the `VictorOps` router if left raw.
///
/// Letters, digits, `-`, `.`, `_`, and `~` are unreserved and pass
/// through unchanged per [RFC 3986 §2.3].
///
/// [RFC 3986 §2.3]: https://www.rfc-editor.org/rfc/rfc3986#section-2.3
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b':')
    .add(b';')
    .add(b'&')
    .add(b'=')
    .add(b'+')
    .add(b'$')
    .add(b',');

/// `VictorOps` / Splunk On-Call provider that posts alerts to the
/// REST endpoint integration.
pub struct VictorOpsProvider {
    config: VictorOpsConfig,
    client: Client,
}

/// Fields extracted from an action payload. Everything except
/// `event_action` is optional at the serde level so the provider
/// can enforce per-branch requirements (e.g., `entity_id` required
/// for `acknowledge` / `resolve`).
#[derive(Debug, Deserialize)]
struct EventPayload {
    event_action: String,
    #[serde(default)]
    routing_key: Option<String>,
    #[serde(default)]
    entity_id: Option<String>,
    #[serde(default)]
    entity_display_name: Option<String>,
    #[serde(default)]
    state_message: Option<String>,
    #[serde(default)]
    host_name: Option<String>,
    #[serde(default)]
    state_start_time: Option<i64>,
    #[serde(default)]
    monitoring_tool: Option<String>,
}

impl VictorOpsProvider {
    /// Create a new `VictorOps` provider with the given configuration.
    ///
    /// Uses a default `reqwest::Client` with a 30-second timeout.
    pub fn new(config: VictorOpsConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new `VictorOps` provider with a custom HTTP client.
    /// Useful for tests and for sharing a connection pool with the
    /// server's other HTTP integrations.
    pub fn with_client(config: VictorOpsConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Percent-encode a value for safe inclusion in an URL path
    /// segment using the [`percent_encoding`] crate so multi-byte
    /// UTF-8, sub-delims, and reserved characters all get handled
    /// per RFC 3986.
    fn percent_encode_path_segment(raw: &str) -> String {
        utf8_percent_encode(raw, PATH_SEGMENT_ENCODE_SET).to_string()
    }

    /// Build the URL for a given routing key.
    ///
    /// Both the `api_key` and the `routing_key` live in the URL
    /// path — the REST endpoint integration's only
    /// authentication mechanism. Both segments are
    /// percent-encoded so an accidentally-special character in
    /// either cannot produce an injection or 404.
    fn integration_url(&self, routing_key: &str) -> String {
        let api_key_seg = Self::percent_encode_path_segment(self.config.api_key());
        let route_seg = Self::percent_encode_path_segment(routing_key);
        format!(
            "{}/integrations/generic/20131114/alert/{api_key_seg}/{route_seg}",
            self.config.api_base_url()
        )
    }

    /// Scope the `entity_id` with `{namespace}:{tenant}:` so two
    /// tenants sharing one `VictorOps` integration key cannot
    /// collide on (or maliciously resolve) each other's alerts.
    /// Matches the `scoped_alias` semantics in `acteon-opsgenie`.
    fn scoped_entity_id(&self, namespace: &str, tenant: &str, raw: &str) -> String {
        if self.config.scope_entity_ids {
            format!("{namespace}:{tenant}:{raw}")
        } else {
            raw.to_owned()
        }
    }

    /// POST a JSON body to the given URL, interpret the response,
    /// and map error statuses onto [`VictorOpsError`].
    async fn post_json<B: serde::Serialize + ?Sized>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<VictorOpsApiResponse, VictorOpsError> {
        // NOTE: do not include the full URL in debug/error output —
        // it contains the api_key and routing_key as path segments.
        debug!("sending request to VictorOps");
        let builder = self.client.post(url).json(body);
        let request = acteon_provider::inject_trace_context(builder);
        let response = request.send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("VictorOps API rate limit hit");
            return Err(VictorOpsError::RateLimited);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let body = response.text().await.unwrap_or_default();
            return Err(VictorOpsError::Unauthorized(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if status.is_server_error() || status == reqwest::StatusCode::REQUEST_TIMEOUT {
            let body = response.text().await.unwrap_or_default();
            warn!(%status, "VictorOps transient error — will be retried by gateway");
            return Err(VictorOpsError::Transient(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(VictorOpsError::Api(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }

        // Non-success response bodies sometimes come back with a
        // sparser JSON shape — fall back to an empty response so
        // the action outcome stays predictable.
        let api_response: VictorOpsApiResponse = response
            .json()
            .await
            .unwrap_or_else(|_| VictorOpsApiResponse::default_fallback());
        Ok(api_response)
    }

    /// Build and send a single alert.
    async fn send_alert(
        &self,
        namespace: &str,
        tenant: &str,
        payload: EventPayload,
    ) -> Result<VictorOpsApiResponse, VictorOpsError> {
        let message_type = VictorOpsMessageType::parse(&payload.event_action)
            .map_err(VictorOpsError::InvalidPayload)?;

        // ack and resolve require an explicit entity_id so the
        // lifecycle event correlates with the original trigger.
        // trigger, warn, and info accept a missing entity_id (the
        // VictorOps server will auto-assign).
        let needs_entity_id = matches!(
            message_type,
            VictorOpsMessageType::Acknowledgement | VictorOpsMessageType::Recovery
        );
        if needs_entity_id && payload.entity_id.is_none() {
            return Err(VictorOpsError::InvalidPayload(format!(
                "{} events require an 'entity_id' field matching the one used at trigger time",
                payload.event_action
            )));
        }

        // Scope the entity_id to prevent cross-tenant collisions on a shared
        // VictorOps integration key.
        let entity_id = payload
            .entity_id
            .map(|raw| self.scoped_entity_id(namespace, tenant, &raw));

        let monitoring_tool = payload
            .monitoring_tool
            .unwrap_or_else(|| self.config.monitoring_tool.clone());

        let routing_key = self
            .config
            .resolve_routing_key(payload.routing_key.as_deref())?
            .to_owned();

        let request = VictorOpsAlertRequest {
            message_type,
            entity_id,
            entity_display_name: payload.entity_display_name,
            state_message: payload.state_message,
            monitoring_tool,
            host_name: payload.host_name,
            state_start_time: payload.state_start_time,
        };
        let url = self.integration_url(&routing_key);
        self.post_json(&url, &request).await
    }
}

impl Provider for VictorOpsProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "victorops"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "victorops"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: EventPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| VictorOpsError::InvalidPayload(format!("failed to parse payload: {e}")))?;
        let namespace = action.namespace.as_str();
        let tenant = action.tenant.as_str();
        let api_response = self.send_alert(namespace, tenant, payload).await?;
        let body = serde_json::json!({
            "result": api_response.result,
            "entity_id": api_response.entity_id,
        });
        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "victorops"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        // VictorOps does not expose a no-op ping. We issue a GET
        // against the base URL — any response (including 404 or
        // 405) means the endpoint is reachable. Only a connection
        // failure counts as a hard health-check error.
        let url = format!(
            "{}/integrations/generic/20131114/alert",
            self.config.api_base_url()
        );
        debug!("performing VictorOps health check");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;
        debug!(status = %response.status(), "VictorOps health check response");
        Ok(())
    }
}

impl VictorOpsApiResponse {
    /// Construct a fallback response used when the server returns
    /// a 2xx with a body we cannot parse. Keeps the action outcome
    /// shape predictable for downstream consumers.
    fn default_fallback() -> Self {
        Self {
            result: "success".into(),
            entity_id: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::VictorOpsConfig;

    /// Tiny mock HTTP server for integration-style tests. Same
    /// pattern the `OpsGenie` provider uses — a single-accept TCP
    /// listener that returns a canned response and captures the
    /// request body so the test can assert on it.
    struct MockVictorOpsServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockVictorOpsServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind mock server");
            let port = listener.local_addr().unwrap().port();
            let base_url = format!("http://127.0.0.1:{port}");
            Self { listener, base_url }
        }

        async fn respond_once(self, status_code: u16, body: &str) {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let _ = stream.read(&mut buf).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }

        async fn respond_once_capturing(self, status_code: u16, body: &str) -> String {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).await.unwrap();
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
            raw
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("incidents", "tenant-1", "victorops", "send_alert", payload)
    }

    #[test]
    fn provider_name() {
        let provider =
            VictorOpsProvider::new(VictorOpsConfig::single_route("api", "team-ops", "rk-ops"));
        assert_eq!(provider.name(), "victorops");
    }

    #[test]
    fn percent_encode_path_segment_escapes_reserved() {
        assert_eq!(
            VictorOpsProvider::percent_encode_path_segment("a/b c:d"),
            "a%2Fb%20c%3Ad"
        );
    }

    #[test]
    fn percent_encode_path_segment_utf8() {
        assert_eq!(
            VictorOpsProvider::percent_encode_path_segment("café"),
            "caf%C3%A9"
        );
    }

    #[tokio::test]
    async fn execute_trigger_success() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("test-api", "team-ops", "rk-ops")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "web-01-high-cpu",
            "entity_display_name": "High CPU on web-01",
            "state_message": "CPU > 90% for 5 minutes.",
            "host_name": "web-01",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(
                    200,
                    r#"{"result":"success","entity_id":"web-01-high-cpu"}"#,
                )
                .await
        });
        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);

        // URL embeds the api_key and routing_key segments, and the
        // entity_id is scoped with {namespace}:{tenant}:.
        assert!(
            request.contains("POST /integrations/generic/20131114/alert/test-api/rk-ops"),
            "URL should embed api_key and routing_key: {request}"
        );
        assert!(request.contains("\"message_type\":\"CRITICAL\""));
        assert!(request.contains("\"entity_id\":\"incidents:tenant-1:web-01-high-cpu\""));
        assert!(request.contains("\"entity_display_name\":\"High CPU on web-01\""));
        assert!(request.contains("\"state_message\":\"CPU > 90% for 5 minutes.\""));
        assert!(request.contains("\"monitoring_tool\":\"acteon\""));
        assert!(request.contains("\"host_name\":\"web-01\""));
    }

    #[tokio::test]
    async fn execute_acknowledge_success() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("test-api", "team-ops", "rk-ops")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "acknowledge",
            "entity_id": "web-01-high-cpu",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success","entity_id":""}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(request.contains("\"message_type\":\"ACKNOWLEDGEMENT\""));
        assert!(request.contains("\"entity_id\":\"incidents:tenant-1:web-01-high-cpu\""));
    }

    #[tokio::test]
    async fn execute_resolve_success() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("test-api", "team-ops", "rk-ops")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "resolve",
            "entity_id": "web-01-high-cpu",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success","entity_id":""}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(request.contains("\"message_type\":\"RECOVERY\""));
    }

    #[tokio::test]
    async fn execute_warn_and_info_message_types() {
        // Single server call per test; `warn` first.
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk-ops")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "warn",
            "entity_id": "disk-warn",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(request.contains("\"message_type\":\"WARNING\""));

        // Fresh server for the info path.
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk-ops")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "info",
            "entity_id": "informational",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(request.contains("\"message_type\":\"INFO\""));
    }

    #[tokio::test]
    async fn execute_acknowledge_missing_entity_id() {
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({ "event_action": "acknowledge" }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_invalid_event_action() {
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "snooze",
            "entity_id": "x",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(429, r#"{"message":"Too Many Requests"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_503_retryable_connection() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(503, r#"{"message":"Service Unavailable"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_unauthorized_maps_to_configuration() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(401, r#"{"message":"Unauthorized"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_api_error_non_retryable() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(400, r#"{"message":"Bad Request"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_with_explicit_routing_key() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::new("api")
            .with_route("team-a", "rk-a")
            .with_route("team-b", "rk-b")
            .with_default_route("team-a")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
            "routing_key": "team-b",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(
            request.contains("POST /integrations/generic/20131114/alert/api/rk-b"),
            "explicit routing_key should pick rk-b: {request}"
        );
    }

    #[tokio::test]
    async fn execute_with_unknown_routing_key_returns_configuration() {
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
            "routing_key": "team-gone",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[tokio::test]
    async fn scoped_entity_id_isolates_tenants() {
        // Two different tenants on the same VictorOps integration
        // key ask to resolve the same raw entity_id. The scoped
        // ids must differ so cross-tenant interference is impossible.
        let provider = VictorOpsProvider::new(VictorOpsConfig::single_route("api", "team", "rk"));
        let a = provider.scoped_entity_id("incidents", "tenant-a", "web-01-high-cpu");
        let b = provider.scoped_entity_id("incidents", "tenant-b", "web-01-high-cpu");
        assert_ne!(a, b);
        assert_eq!(a, "incidents:tenant-a:web-01-high-cpu");
        assert_eq!(b, "incidents:tenant-b:web-01-high-cpu");
    }

    #[tokio::test]
    async fn scope_disabled_passes_through() {
        let config =
            VictorOpsConfig::single_route("api", "team", "rk").with_scope_entity_ids(false);
        let provider = VictorOpsProvider::new(config);
        assert_eq!(
            provider.scoped_entity_id("incidents", "tenant-1", "raw-id"),
            "raw-id"
        );
    }

    #[tokio::test]
    async fn execute_with_scope_entity_ids_disabled() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url)
            .with_scope_entity_ids(false);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "raw-id",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(
            request.contains("\"entity_id\":\"raw-id\""),
            "opt-out should pass entity_id through unchanged: {request}"
        );
    }

    #[tokio::test]
    async fn execute_override_monitoring_tool() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url)
            .with_monitoring_tool("acteon-test");
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_id": "x",
            "monitoring_tool": "prometheus",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        // Payload override wins over config default.
        assert!(request.contains("\"monitoring_tool\":\"prometheus\""));
    }

    #[tokio::test]
    async fn execute_trigger_allows_missing_entity_id() {
        let server = MockVictorOpsServer::start().await;
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk")
            .with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "trigger",
            "entity_display_name": "Quick alert",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"result":"success","entity_id":"auto"}"#)
                .await
        });
        let result = provider.execute(&action).await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn integration_url_embeds_both_secrets() {
        let provider = VictorOpsProvider::new(VictorOpsConfig::single_route(
            "org-api-secret",
            "team-ops",
            "route-secret",
        ));
        let url = provider.integration_url("route-secret");
        assert_eq!(
            url,
            "https://alert.victorops.com/integrations/generic/20131114/alert/org-api-secret/route-secret"
        );
    }

    #[tokio::test]
    async fn integration_url_percent_encodes_segments() {
        let provider = VictorOpsProvider::new(VictorOpsConfig::single_route(
            "org/api",
            "team",
            "route key",
        ));
        // Both path segments get encoded independently so neither
        // can inject an additional path component.
        let url = provider.integration_url("route key");
        assert_eq!(
            url,
            "https://alert.victorops.com/integrations/generic/20131114/alert/org%2Fapi/route%20key"
        );
    }

    #[tokio::test]
    async fn health_check_reachable_endpoint() {
        let server = MockVictorOpsServer::start().await;
        let config =
            VictorOpsConfig::single_route("api", "team", "rk").with_api_base_url(&server.base_url);
        let provider = VictorOpsProvider::new(config);
        // Even a 404 means the endpoint is reachable.
        let server_handle = tokio::spawn(async move { server.respond_once(404, "{}").await });
        let result = provider.health_check().await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_connection_failure() {
        let config = VictorOpsConfig::single_route("api", "team", "rk")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = VictorOpsProvider::new(config);
        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }
}
