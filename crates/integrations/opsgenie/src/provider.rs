use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError, truncate_error_body};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::OpsGenieConfig;
use crate::error::OpsGenieError;
use crate::types::{
    OpsGenieAlertRequest, OpsGenieApiResponse, OpsGenieLifecycleRequest, OpsGenieResponder,
};

/// Characters that must be percent-encoded inside an URL path
/// segment. We start from the IETF `CONTROLS` set and add every
/// sub-delim, query-start, and path-separator byte that would be
/// misinterpreted by the `OpsGenie` router if left raw.
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

/// `OpsGenie` provider that creates, acknowledges, and closes alerts
/// via the Alert API v2.
pub struct OpsGenieProvider {
    config: OpsGenieConfig,
    client: Client,
}

/// Fields extracted from an action payload. Everything except
/// `event_action` is optional at the serde level so the provider can
/// choose which ones are mandatory per `event_action` variant.
#[derive(Debug, Deserialize)]
struct EventPayload {
    event_action: String,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    details: Option<serde_json::Value>,
    #[serde(default)]
    actions: Option<Vec<String>>,
    #[serde(default)]
    responders: Option<Vec<OpsGenieResponder>>,
    #[serde(default)]
    visible_to: Option<Vec<OpsGenieResponder>>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

impl OpsGenieProvider {
    /// Create a new `OpsGenie` provider with the given configuration.
    ///
    /// Uses a default `reqwest::Client` with a 30-second timeout.
    pub fn new(config: OpsGenieConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new `OpsGenie` provider with a custom HTTP client.
    ///
    /// Useful for testing or for sharing a connection pool with the
    /// server's other HTTP integrations.
    pub fn with_client(config: OpsGenieConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Build the URL for the `POST /v2/alerts` create endpoint.
    fn create_url(&self) -> String {
        format!("{}/v2/alerts", self.config.api_base_url())
    }

    /// Build the URL for the `POST /v2/alerts/{alias}/{action}` endpoint.
    fn lifecycle_url(&self, alias: &str, action: &str) -> String {
        // We exclusively use alias-based lookups because the
        // `identifier` path parameter is interpreted as an alert ID
        // by default. `identifierType=alias` flips that so the same
        // alias the dispatcher used to create the alert works for
        // ack/close too.
        let alias = Self::percent_encode_path_segment(alias);
        format!(
            "{}/v2/alerts/{alias}/{action}?identifierType=alias",
            self.config.api_base_url()
        )
    }

    /// Percent-encode a value for safe inclusion in an URL path
    /// segment, using the battle-tested [`percent_encoding`] crate
    /// so multi-byte UTF-8, sub-delims, and reserved characters
    /// all get handled per RFC 3986 without the provider having
    /// to maintain its own encoding table.
    fn percent_encode_path_segment(raw: &str) -> String {
        utf8_percent_encode(raw, PATH_SEGMENT_ENCODE_SET).to_string()
    }

    /// POST a JSON body to the given URL, interpret the response, and
    /// map error statuses onto [`OpsGenieError`].
    async fn post_json<B: serde::Serialize + ?Sized>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<OpsGenieApiResponse, OpsGenieError> {
        debug!(url = %url, "sending request to OpsGenie");
        let builder = self
            .client
            .post(url)
            .header(
                "Authorization",
                format!("GenieKey {}", self.config.api_key()),
            )
            .json(body);
        let request = acteon_provider::inject_trace_context(builder);
        let response = request.send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("OpsGenie API rate limit hit");
            return Err(OpsGenieError::RateLimited);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let body = response.text().await.unwrap_or_default();
            return Err(OpsGenieError::Unauthorized(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        // 5xx (server error) and 408 (Request Timeout) are
        // transient: the request body was fine, the server was
        // temporarily unable to handle it. These must be retried
        // rather than dropped, otherwise a 10-second OpsGenie blip
        // would permanently lose alerts.
        if status.is_server_error() || status == reqwest::StatusCode::REQUEST_TIMEOUT {
            let body = response.text().await.unwrap_or_default();
            warn!(%status, "OpsGenie transient error — will be retried by gateway");
            return Err(OpsGenieError::Transient(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(OpsGenieError::Api(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }

        // Non-success response bodies from OpsGenie are JSON but
        // tolerant parsing is still safer than failing the action if
        // the server returns an unexpected shape on a 2xx.
        let api_response: OpsGenieApiResponse = response
            .json()
            .await
            .unwrap_or(OpsGenieApiResponse::default_fallback());
        Ok(api_response)
    }

    /// Scope a user-supplied alias with the action's
    /// `(namespace, tenant)` pair so two different tenants sharing
    /// a single `OpsGenie` integration key cannot collide on (or
    /// maliciously close) each other's alerts by guessing an
    /// alias. The prefix is applied consistently to `create`,
    /// `acknowledge`, and `close` so all three resolve to the same
    /// underlying `OpsGenie` incident.
    ///
    /// Operators who genuinely want an unscoped alias (e.g.,
    /// single-tenant deployments, or cross-tenant coordination)
    /// can set `scope_aliases = false` in the provider config.
    fn scoped_alias(&self, namespace: &str, tenant: &str, raw: &str) -> String {
        if self.config.scope_aliases {
            format!("{namespace}:{tenant}:{raw}")
        } else {
            raw.to_owned()
        }
    }

    /// Handle the `event_action = "create"` branch.
    async fn execute_create(
        &self,
        namespace: &str,
        tenant: &str,
        payload: EventPayload,
    ) -> Result<OpsGenieApiResponse, OpsGenieError> {
        let message = payload.message.ok_or_else(|| {
            OpsGenieError::InvalidPayload("create events require a 'message' field".into())
        })?;
        let mut message = message;
        let max_len = self.config.message_max_length;
        if message.len() > max_len {
            message.truncate(max_len);
        }

        // Default the responder to the configured team when no
        // responders are provided. A missing default team is fine —
        // the account's routing rules handle unrouted alerts.
        let responders = payload.responders.or_else(|| {
            self.config.default_team.as_ref().map(|team| {
                vec![OpsGenieResponder {
                    name: Some(team.clone()),
                    id: None,
                    username: None,
                    kind: "team".into(),
                }]
            })
        });

        let priority = payload
            .priority
            .unwrap_or_else(|| self.config.default_priority.clone());
        let source = payload
            .source
            .or_else(|| self.config.default_source.clone());

        // Scope the alias to prevent cross-tenant collisions on a
        // shared OpsGenie account.
        let alias = payload
            .alias
            .map(|raw| self.scoped_alias(namespace, tenant, &raw));

        let request = OpsGenieAlertRequest {
            message,
            alias,
            description: payload.description,
            responders,
            visible_to: payload.visible_to,
            actions: payload.actions,
            tags: payload.tags,
            details: payload.details,
            entity: payload.entity,
            source,
            priority: Some(priority),
            user: payload.user,
            note: payload.note,
        };
        let url = self.create_url();
        self.post_json(&url, &request).await
    }

    /// Handle the `event_action = "acknowledge"` / `"close"` branches.
    async fn execute_lifecycle(
        &self,
        namespace: &str,
        tenant: &str,
        action: &str,
        payload: EventPayload,
    ) -> Result<OpsGenieApiResponse, OpsGenieError> {
        let raw_alias = payload.alias.ok_or_else(|| {
            OpsGenieError::InvalidPayload(format!(
                "{action} events require an 'alias' field that matches the alias used at create time"
            ))
        })?;
        // Apply the same tenant-scope prefix used at create time so
        // the ack/close request targets the correct incident.
        let alias = self.scoped_alias(namespace, tenant, &raw_alias);
        let request = OpsGenieLifecycleRequest {
            source: payload
                .source
                .or_else(|| self.config.default_source.clone()),
            user: payload.user,
            note: payload.note,
        };
        let url = self.lifecycle_url(&alias, action);
        self.post_json(&url, &request).await
    }
}

impl Provider for OpsGenieProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "opsgenie"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "opsgenie"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: EventPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| OpsGenieError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let namespace = action.namespace.as_str();
        let tenant = action.tenant.as_str();
        let api_response = match payload.event_action.as_str() {
            "create" => self.execute_create(namespace, tenant, payload).await?,
            "acknowledge" => {
                self.execute_lifecycle(namespace, tenant, "acknowledge", payload)
                    .await?
            }
            "close" => {
                self.execute_lifecycle(namespace, tenant, "close", payload)
                    .await?
            }
            other => {
                return Err(OpsGenieError::InvalidPayload(format!(
                    "invalid event_action '{other}': must be 'create', 'acknowledge', or 'close'"
                ))
                .into());
            }
        };

        let body = serde_json::json!({
            "result": api_response.result,
            "took": api_response.took,
            "request_id": api_response.request_id,
        });
        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "opsgenie"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        // OpsGenie does not expose a public ping endpoint, so we reuse
        // the Alert API base URL the same way the PagerDuty provider
        // does: any response (including 400/401/403) proves the
        // endpoint is reachable. Only a connection failure counts as a
        // hard health-check error.
        let url = format!("{}/v2/alerts", self.config.api_base_url());
        debug!("performing OpsGenie health check");
        let response = self
            .client
            .get(&url)
            .header(
                "Authorization",
                format!("GenieKey {}", self.config.api_key()),
            )
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;
        debug!(status = %response.status(), "OpsGenie health check response");
        Ok(())
    }
}

impl OpsGenieApiResponse {
    /// Construct a fallback response used when the server returns a
    /// 2xx with a body we cannot parse. Keeps the action outcome
    /// shape predictable for downstream consumers.
    fn default_fallback() -> Self {
        Self {
            result: "Request accepted".into(),
            took: 0.0,
            request_id: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::{DEFAULT_MESSAGE_MAX_LENGTH, OpsGenieConfig, OpsGenieRegion};

    /// Tiny mock HTTP server for integration-style tests. Same
    /// pattern the `PagerDuty` provider uses — a single-accept TCP
    /// listener that returns a canned response and captures the
    /// request body so the test can assert on it.
    struct MockOpsGenieServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockOpsGenieServer {
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
            let request = raw.clone();
            let response = format!(
                "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
            request
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("incidents", "tenant-1", "opsgenie", "send_alert", payload)
    }

    #[test]
    fn provider_name() {
        let provider = OpsGenieProvider::new(OpsGenieConfig::new("test-key"));
        assert_eq!(provider.name(), "opsgenie");
    }

    #[test]
    fn percent_encode_path_segment_passthrough() {
        assert_eq!(
            OpsGenieProvider::percent_encode_path_segment("web-01_high.cpu"),
            "web-01_high.cpu"
        );
    }

    #[test]
    fn percent_encode_path_segment_escapes_reserved() {
        // Slash, colon, and space are reserved in path segments.
        assert_eq!(
            OpsGenieProvider::percent_encode_path_segment("a/b c:d"),
            "a%2Fb%20c%3Ad"
        );
    }

    #[test]
    fn percent_encode_path_segment_handles_utf8_multibyte() {
        // A multi-byte UTF-8 character (`é` = 0xC3 0xA9) must be
        // encoded as two `%XX` escapes, not mis-decoded. The old
        // hand-rolled encoder worked byte-by-byte, so this is a
        // regression guard against re-introducing a bug there.
        assert_eq!(
            OpsGenieProvider::percent_encode_path_segment("café"),
            "caf%C3%A9"
        );
    }

    #[test]
    fn percent_encode_path_segment_preserves_unreserved() {
        // RFC 3986 unreserved set must pass through verbatim.
        assert_eq!(
            OpsGenieProvider::percent_encode_path_segment("abc-DEF_123.tilde~"),
            "abc-DEF_123.tilde~"
        );
    }

    #[tokio::test]
    async fn execute_create_success() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key")
            .with_region(OpsGenieRegion::Us)
            .with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "High CPU on web-01",
            "alias": "web-01-high-cpu",
            "priority": "P2",
            "tags": ["cpu", "critical"],
        }));
        let response_body =
            r#"{"result":"Request will be processed","took":0.123,"requestId":"req-abc"}"#;
        let server_handle =
            tokio::spawn(async move { server.respond_once_capturing(202, response_body).await });

        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["result"], "Request will be processed");
        assert_eq!(response.body["request_id"], "req-abc");

        // The dispatched request should hit POST /v2/alerts with the
        // `Authorization: GenieKey ...` header and a JSON body that
        // contains the payload fields. Alias is scoped by default
        // with `{namespace}:{tenant}:` so tenant isolation holds on
        // a shared OpsGenie integration.
        assert!(request.contains("POST /v2/alerts"));
        assert!(request.contains("authorization: GenieKey test-key"));
        assert!(request.contains("\"message\":\"High CPU on web-01\""));
        assert!(request.contains("\"alias\":\"incidents:tenant-1:web-01-high-cpu\""));
        assert!(request.contains("\"priority\":\"P2\""));
    }

    #[tokio::test]
    async fn execute_create_uses_default_team_and_priority() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key")
            .with_api_base_url(&server.base_url)
            .with_default_team("platform-oncall")
            .with_default_priority("P1")
            .with_default_source("acteon");
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "Latency spike",
        }));
        let response_body = r#"{"result":"ok","took":0.01,"requestId":"r1"}"#;
        let server_handle =
            tokio::spawn(async move { server.respond_once_capturing(202, response_body).await });

        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        assert!(result.is_ok());
        // Default team materialized into the responders list.
        assert!(
            request.contains("\"responders\":[{\"name\":\"platform-oncall\",\"type\":\"team\"}]"),
            "defaults should produce a team responder: {request}"
        );
        // Default priority used when payload omits it.
        assert!(request.contains("\"priority\":\"P1\""));
        // Default source materialized into the body.
        assert!(request.contains("\"source\":\"acteon\""));
    }

    #[tokio::test]
    async fn execute_create_truncates_long_message() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let long = "x".repeat(200);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": long,
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(202, r#"{"result":"ok","took":0.0,"requestId":""}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        // Body should contain exactly DEFAULT_MESSAGE_MAX_LENGTH x's.
        let marker = format!("\"message\":\"{}\"", "x".repeat(DEFAULT_MESSAGE_MAX_LENGTH));
        assert!(
            request.contains(&marker),
            "message should be truncated to {DEFAULT_MESSAGE_MAX_LENGTH}"
        );
    }

    #[tokio::test]
    async fn execute_create_respects_configured_message_max_length() {
        let server = MockOpsGenieServer::start().await;
        // Pretend OpsGenie raised the cap to 200 and we want to
        // use the full width.
        let config = OpsGenieConfig::new("test-key")
            .with_api_base_url(&server.base_url)
            .with_message_max_length(200);
        let provider = OpsGenieProvider::new(config);
        let long = "y".repeat(250);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": long,
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(202, r#"{"result":"ok","took":0.0,"requestId":""}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        let marker = format!("\"message\":\"{}\"", "y".repeat(200));
        assert!(
            request.contains(&marker),
            "configured max length (200) must override the default"
        );
    }

    #[tokio::test]
    async fn execute_create_missing_message() {
        let config = OpsGenieConfig::new("k").with_api_base_url("http://127.0.0.1:1");
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({ "event_action": "create" }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_acknowledge_success() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "acknowledge",
            "alias": "web-01-high-cpu",
            "note": "picked up by oncall",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(
                    202,
                    r#"{"result":"Request will be processed","took":0.01,"requestId":"r2"}"#,
                )
                .await
        });
        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        assert!(result.is_ok());
        // The alias is scoped with {namespace}:{tenant}: and
        // percent-encoded in the path segment (colons → %3A) so
        // the URL points at the same incident that `create`
        // registered.
        assert!(
            request.contains(
                "POST /v2/alerts/incidents%3Atenant-1%3Aweb-01-high-cpu/acknowledge?identifierType=alias"
            ),
            "expected alias-based ack URL with scope prefix: {request}"
        );
        assert!(request.contains("\"note\":\"picked up by oncall\""));
    }

    #[tokio::test]
    async fn execute_close_success() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "close",
            "alias": "db-backup-failed",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(
                    202,
                    r#"{"result":"Request will be processed","took":0.01,"requestId":"r3"}"#,
                )
                .await
        });
        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        assert!(result.is_ok());
        assert!(
            request.contains(
                "POST /v2/alerts/incidents%3Atenant-1%3Adb-backup-failed/close?identifierType=alias"
            ),
            "expected alias-based close URL with scope prefix: {request}"
        );
    }

    #[tokio::test]
    async fn scoped_alias_isolates_tenants_cross_tenant_ack_does_not_land() {
        // Two different tenants on the same OpsGenie integration
        // key ask to acknowledge the same raw alias. Their scoped
        // aliases must differ so one cannot close the other's
        // incident by guessing the alias.
        let provider = OpsGenieProvider::new(OpsGenieConfig::new("k"));
        let a = provider.scoped_alias("incidents", "tenant-a", "web-01-high-cpu");
        let b = provider.scoped_alias("incidents", "tenant-b", "web-01-high-cpu");
        assert_ne!(a, b, "two tenants must not collide on the same raw alias");
        assert_eq!(a, "incidents:tenant-a:web-01-high-cpu");
        assert_eq!(b, "incidents:tenant-b:web-01-high-cpu");
    }

    #[tokio::test]
    async fn scope_aliases_disabled_returns_raw_alias() {
        let provider = OpsGenieProvider::new(OpsGenieConfig::new("k").with_scope_aliases(false));
        let alias = provider.scoped_alias("incidents", "tenant-1", "web-01-high-cpu");
        assert_eq!(alias, "web-01-high-cpu");
    }

    #[tokio::test]
    async fn execute_create_with_scope_aliases_disabled() {
        // Opt-out path: the alias passes through unchanged so
        // single-tenant deployments (or cross-tenant coordination
        // scenarios) can use raw OpsGenie aliases.
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key")
            .with_api_base_url(&server.base_url)
            .with_scope_aliases(false);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "Raw alias test",
            "alias": "web-01-high-cpu",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(202, r#"{"result":"ok","took":0.0,"requestId":""}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(
            request.contains("\"alias\":\"web-01-high-cpu\""),
            "opt-out should pass the alias through unchanged: {request}"
        );
    }

    #[tokio::test]
    async fn execute_acknowledge_missing_alias() {
        let config = OpsGenieConfig::new("k").with_api_base_url("http://127.0.0.1:1");
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({ "event_action": "acknowledge" }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_invalid_event_action() {
        let config = OpsGenieConfig::new("k").with_api_base_url("http://127.0.0.1:1");
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "snooze",
            "alias": "x",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "test",
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
    async fn execute_unauthorized_maps_to_configuration() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "test",
        }));
        let server_handle =
            tokio::spawn(async move { server.respond_once(401, r#"{"message":"unauth"}"#).await });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(
            matches!(err, ProviderError::Configuration(_)),
            "401 should surface as Configuration, got {err:?}"
        );
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_5xx_maps_to_retryable_connection() {
        // A brief OpsGenie outage (503 Service Unavailable) must
        // surface as a retryable error so the gateway re-queues the
        // alert rather than permanently dropping it.
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "test",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(503, r#"{"message":"Service Unavailable"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(
            matches!(err, ProviderError::Connection(_)),
            "503 should surface as Connection, got {err:?}"
        );
        assert!(
            err.is_retryable(),
            "503 errors must be retryable (gateway re-queues)"
        );
    }

    #[tokio::test]
    async fn execute_502_maps_to_retryable_connection() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "test",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(502, r#"{"message":"Bad Gateway"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_api_error_non_retryable() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "create",
            "message": "test",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(400, r#"{"message":"Request body is invalid"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_reachable_endpoint() {
        let server = MockOpsGenieServer::start().await;
        let config = OpsGenieConfig::new("test-key").with_api_base_url(&server.base_url);
        let provider = OpsGenieProvider::new(config);
        // Even a 400/401 means the endpoint is reachable.
        let server_handle =
            tokio::spawn(async move { server.respond_once(401, r#"{"message":"no"}"#).await });
        let result = provider.health_check().await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_connection_failure() {
        let config = OpsGenieConfig::new("test-key").with_api_base_url("http://127.0.0.1:1");
        let provider = OpsGenieProvider::new(config);
        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }
}
