//! A2A Task push-notification configurations (Phase 4 of the A2A
//! protocol). A push config binds a webhook URL to a single A2A Task;
//! when the task emits a streaming event (transition, history append,
//! artifact update) the delivery worker POSTs the event envelope to
//! every config registered for that task.
//!
//! The config row is the durable, addressable record — CRUD over
//! JSON-RPC (`tasks/pushNotificationConfig/{set,get,list,delete}`)
//! and REST (`/v1/tasks/{id}/pushNotificationConfigs[/{cfgId}]`). The
//! delivery worker is a separate module that consumes the gateway's
//! stream broadcast.
//!
//! Storage layout (see `KeyKind::A2aTaskPushConfig`):
//! - one row per config, addressed `{task_id}:{config_id}`;
//! - `scan_keys` with a `task_id:` prefix lists every config bound to
//!   the task in one call (and so `delete_task` can sweep all of them
//!   without a separate index row).
//!
//! Validation is conservative: HTTPS-only by default (an opt-in flag
//! is reserved for local-loop testing in the delivery worker, not on
//! the config), a hard cap on URL length, and an explicit deny-list
//! for non-`http(s)` schemes. The bearer token, if present, is treated
//! as a secret and never logged.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Hard cap on the size of a single DLQ entry's serialized event
/// payload. The worker uses the cap to truncate oversized events
/// rather than blocking writes — a truncated payload is still
/// useful for diagnostics.
pub const MAX_DLQ_EVENT_BYTES: usize = 32 * 1024;

/// Hard cap on the size of a single DLQ entry's `last_error`
/// message. A misbehaving server can return arbitrarily long error
/// bodies; the cap keeps the DLQ row bounded.
pub const MAX_DLQ_ERROR_BYTES: usize = 4 * 1024;

/// Why a delivery landed in the DLQ. Used to drive operator
/// remediation: `Terminal` means the URL is permanently rejecting
/// the payload (configuration problem), `Exhausted` means the URL
/// is intermittently failing (capacity / network problem).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum DlqFailureKind {
    /// HTTP 4xx outside the transient set — the URL is permanently
    /// rejecting the payload. Operator should fix the config and
    /// delete + re-create.
    Terminal,
    /// The transient retry budget was exhausted without success.
    /// Operator can replay the entry once the underlying service is
    /// healthy.
    Exhausted,
}

/// One Dead-Letter-Queue entry recorded when a push-notification
/// delivery exhausts its retry budget or is permanently rejected by
/// the receiver. Persisted at `KeyKind::A2aPushDlq` keyed
/// `{task_id}:{entry_id}` so a prefix-scan by task id lists every
/// failed delivery for one task.
///
/// The `Debug` impl redacts both `event_json` and `last_error`
/// because each can carry tenant payload bytes a log consumer wasn't
/// authorized to see.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PushDeliveryDlqEntry {
    /// DLQ-entry identifier (`UUIDv7` by convention).
    pub id: String,
    /// The push config whose delivery failed.
    pub config_id: String,
    /// The Task the event was scoped to.
    pub task_id: String,
    /// Namespace + tenant of the task. Stored on the row so a
    /// cross-tenant DLQ scan stays self-describing.
    pub namespace: String,
    /// Tenant of the task.
    pub tenant: String,
    /// URL the worker tried to POST to at the moment of failure.
    /// Snapshotted because the live config may have been edited
    /// since.
    pub url: String,
    /// Whether the failure is terminal or just exhausted.
    pub failure_kind: DlqFailureKind,
    /// Last error observed (HTTP status + reason or network
    /// description). Capped at [`MAX_DLQ_ERROR_BYTES`].
    pub last_error: String,
    /// Number of attempts the worker made before giving up.
    pub attempts: u32,
    /// Wall-clock time of the first failed attempt.
    pub first_failed_at: DateTime<Utc>,
    /// Wall-clock time of the last failed attempt (i.e., when the
    /// row was written).
    pub last_failed_at: DateTime<Utc>,
    /// Serialized `StreamEvent` envelope. Truncated to
    /// [`MAX_DLQ_EVENT_BYTES`]; truncated entries get a marker
    /// suffix so consumers can detect it.
    pub event_json: String,
}

impl fmt::Debug for PushDeliveryDlqEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushDeliveryDlqEntry")
            .field("id", &self.id)
            .field("config_id", &self.config_id)
            .field("task_id", &self.task_id)
            .field("namespace", &self.namespace)
            .field("tenant", &self.tenant)
            .field("url", &self.url)
            .field("failure_kind", &self.failure_kind)
            .field(
                "last_error",
                &format!("[REDACTED {} bytes]", self.last_error.len()),
            )
            .field("attempts", &self.attempts)
            .field("first_failed_at", &self.first_failed_at)
            .field("last_failed_at", &self.last_failed_at)
            .field(
                "event_json",
                &format!("[REDACTED {} bytes]", self.event_json.len()),
            )
            .finish()
    }
}

impl PushDeliveryDlqEntry {
    /// Build a new DLQ entry. `event_json` and `last_error` are
    /// truncated to their respective caps so a misbehaving event
    /// or error body can't blow up the DLQ row size.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // 1:1 with the persisted row
    pub fn new(
        id: impl Into<String>,
        config_id: impl Into<String>,
        task_id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        url: impl Into<String>,
        failure_kind: DlqFailureKind,
        last_error: impl Into<String>,
        attempts: u32,
        event_json: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            config_id: config_id.into(),
            task_id: task_id.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            url: url.into(),
            failure_kind,
            last_error: truncate(last_error.into(), MAX_DLQ_ERROR_BYTES),
            attempts,
            first_failed_at: now,
            last_failed_at: now,
            event_json: truncate(event_json, MAX_DLQ_EVENT_BYTES),
        }
    }

    /// Storage key shape: `{task_id}:{entry_id}`. The `task_id`
    /// prefix lets operator tooling list every DLQ entry for one
    /// task in a single `scan_keys` call.
    #[must_use]
    pub fn storage_id(&self) -> String {
        format!("{}:{}", self.task_id, self.id)
    }
}

/// Truncate `s` to at most `cap` bytes, appending a marker suffix
/// when truncation actually happened. Splits on a char boundary so
/// a multi-byte UTF-8 codepoint at the boundary isn't sliced in
/// half.
fn truncate(mut s: String, cap: usize) -> String {
    if s.len() <= cap {
        return s;
    }
    // Walk back to a char boundary; UTF-8 codepoints are at most 4
    // bytes so the walk is bounded.
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s.push_str("…[truncated]");
    s
}

/// Maximum length of a push-notification URL. A generous cap that
/// still bounds the storage row — webhook URLs longer than this in
/// the wild are almost always misconfiguration.
pub const MAX_PUSH_URL_BYTES: usize = 2_048;

/// Maximum length of the optional bearer token. Bound at 4 KiB so a
/// caller cannot smuggle a multi-MB blob through this field.
pub const MAX_PUSH_TOKEN_BYTES: usize = 4_096;

/// Maximum length of a free-form security-scheme alias the caller can
/// pin onto a config. The schemes themselves are referenced by name —
/// not stored inline — so the cap is small.
pub const MAX_PUSH_SCHEME_ALIAS_BYTES: usize = 256;

/// Maximum number of security-scheme aliases per config. Long lists
/// here are almost always a misconfiguration.
pub const MAX_PUSH_SCHEMES_PER_CONFIG: usize = 8;

/// Validation failures for a [`TaskPushNotificationConfig`]. Reported
/// to the caller as `invalid_params` over JSON-RPC and `400 Bad
/// Request` over REST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPushConfigValidationError {
    /// A required identity field (`id`, `task_id`, `namespace`,
    /// `tenant`) was empty.
    MissingField(&'static str),
    /// The URL field was empty.
    EmptyUrl,
    /// The URL field exceeded [`MAX_PUSH_URL_BYTES`].
    UrlTooLong(usize),
    /// The URL did not start with `http://` or `https://`.
    UnsupportedScheme(String),
    /// The token field exceeded [`MAX_PUSH_TOKEN_BYTES`].
    TokenTooLong(usize),
    /// The authentication schemes list exceeded
    /// [`MAX_PUSH_SCHEMES_PER_CONFIG`].
    TooManySchemes(usize),
    /// One of the authentication scheme aliases exceeded
    /// [`MAX_PUSH_SCHEME_ALIAS_BYTES`].
    SchemeAliasTooLong(usize),
}

impl fmt::Display for TaskPushConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(name) => write!(f, "{name} must not be empty"),
            Self::EmptyUrl => write!(f, "url must not be empty"),
            Self::UrlTooLong(n) => write!(
                f,
                "url must be at most {MAX_PUSH_URL_BYTES} bytes (got {n})"
            ),
            Self::UnsupportedScheme(s) => {
                write!(
                    f,
                    "url scheme '{s}' is not supported (use http:// or https://)"
                )
            }
            Self::TokenTooLong(n) => write!(
                f,
                "token must be at most {MAX_PUSH_TOKEN_BYTES} bytes (got {n})"
            ),
            Self::TooManySchemes(n) => write!(
                f,
                "at most {MAX_PUSH_SCHEMES_PER_CONFIG} authentication schemes per config (got {n})"
            ),
            Self::SchemeAliasTooLong(n) => write!(
                f,
                "scheme alias must be at most {MAX_PUSH_SCHEME_ALIAS_BYTES} bytes (got {n})"
            ),
        }
    }
}

impl std::error::Error for TaskPushConfigValidationError {}

/// Optional authentication metadata attached to a push-notification
/// config. Mirrors the A2A spec's `PushNotificationAuthenticationInfo`:
/// a list of security-scheme aliases (which the receiving webhook is
/// expected to recognize) and an optional credentials blob.
///
/// The `credentials` field is a free-form string — the spec leaves the
/// shape up to the scheme. Acteon treats it as a secret: it is never
/// logged and is redacted in `Debug` output.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PushAuthentication {
    /// Aliases of `securitySchemes` (as published on the `AgentCard`)
    /// that the receiving webhook supports. Empty list means "no
    /// preference — try the embedded credentials directly."
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemes: Vec<String>,
    /// Opaque credentials blob. Shape is scheme-defined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
}

impl fmt::Debug for PushAuthentication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushAuthentication")
            .field("schemes", &self.schemes)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// A push-notification config row for one A2A Task. Bind one or many
/// of these to a task via `tasks/pushNotificationConfig/set`; the
/// delivery worker fans every streamed event out to each registered
/// URL.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskPushNotificationConfig {
    /// Config-level identifier. Distinct from `task_id` so the same
    /// task can carry several configs (e.g. one webhook per
    /// notification audience). `UUIDv7` by convention.
    pub id: String,
    /// The A2A Task this config is bound to.
    pub task_id: String,
    /// Namespace the task lives in. Stored on the row so the row is
    /// self-describing when read out of a multi-tenant scan.
    pub namespace: String,
    /// Tenant the task lives in.
    pub tenant: String,
    /// Destination URL the delivery worker POSTs streamed events to.
    /// Must be `http://` or `https://`.
    pub url: String,
    /// Optional bearer token sent in an `Authorization: Bearer …`
    /// header on every POST. Treated as a secret — never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Optional richer authentication metadata pointing at one or
    /// more `securitySchemes` on the `AgentCard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<PushAuthentication>,
    /// Wall-clock time the row was first created.
    pub created_at: DateTime<Utc>,
    /// Wall-clock time the row was last updated. Matches `created_at`
    /// on a freshly-minted config.
    pub updated_at: DateTime<Utc>,
}

impl fmt::Debug for TaskPushNotificationConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskPushNotificationConfig")
            .field("id", &self.id)
            .field("task_id", &self.task_id)
            .field("namespace", &self.namespace)
            .field("tenant", &self.tenant)
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("authentication", &self.authentication)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl TaskPushNotificationConfig {
    /// Build a fresh config with `created_at == updated_at == now`.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        task_id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            task_id: task_id.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            url: url.into(),
            token: None,
            authentication: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Storage key shape: `{task_id}:{config_id}`. The `task_id`
    /// prefix lets the list endpoint walk every config for one task
    /// in a single `scan_keys` call.
    #[must_use]
    pub fn storage_id(&self) -> String {
        format!("{}:{}", self.task_id, self.id)
    }

    /// Validate the row against the structural limits in this module.
    /// Returns the first violation found.
    pub fn validate(&self) -> Result<(), TaskPushConfigValidationError> {
        if self.id.is_empty() {
            return Err(TaskPushConfigValidationError::MissingField("id"));
        }
        if self.task_id.is_empty() {
            return Err(TaskPushConfigValidationError::MissingField("task_id"));
        }
        if self.namespace.is_empty() {
            return Err(TaskPushConfigValidationError::MissingField("namespace"));
        }
        if self.tenant.is_empty() {
            return Err(TaskPushConfigValidationError::MissingField("tenant"));
        }
        if self.url.is_empty() {
            return Err(TaskPushConfigValidationError::EmptyUrl);
        }
        if self.url.len() > MAX_PUSH_URL_BYTES {
            return Err(TaskPushConfigValidationError::UrlTooLong(self.url.len()));
        }
        // Accept http(s) only. Other schemes (file://, gopher://,
        // javascript:) have caused server-side request-forgery and
        // information-disclosure incidents in similar webhook
        // products; deny them up front.
        let scheme = self
            .url
            .split_once(':')
            .map(|(s, _)| s.to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(scheme.as_str(), "http" | "https") {
            return Err(TaskPushConfigValidationError::UnsupportedScheme(scheme));
        }
        if let Some(t) = &self.token
            && t.len() > MAX_PUSH_TOKEN_BYTES
        {
            return Err(TaskPushConfigValidationError::TokenTooLong(t.len()));
        }
        if let Some(auth) = &self.authentication {
            if auth.schemes.len() > MAX_PUSH_SCHEMES_PER_CONFIG {
                return Err(TaskPushConfigValidationError::TooManySchemes(
                    auth.schemes.len(),
                ));
            }
            for s in &auth.schemes {
                if s.len() > MAX_PUSH_SCHEME_ALIAS_BYTES {
                    return Err(TaskPushConfigValidationError::SchemeAliasTooLong(s.len()));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(id: &str, task_id: &str, url: &str) -> TaskPushNotificationConfig {
        TaskPushNotificationConfig::new(id, task_id, "agents", "demo", url)
    }

    #[test]
    fn validate_accepts_https_url() {
        let c = fresh("c1", "t1", "https://example.org/hook");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn validate_accepts_http_url() {
        let c = fresh("c1", "t1", "http://localhost:9000/hook");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn validate_rejects_javascript_scheme() {
        let c = fresh("c1", "t1", "javascript:alert(1)");
        let err = c.validate().unwrap_err();
        assert!(matches!(
            err,
            TaskPushConfigValidationError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn validate_rejects_empty_url() {
        let c = fresh("c1", "t1", "");
        assert!(matches!(
            c.validate(),
            Err(TaskPushConfigValidationError::EmptyUrl)
        ));
    }

    #[test]
    fn validate_rejects_oversize_url() {
        let url = format!("https://example.org/{}", "a".repeat(MAX_PUSH_URL_BYTES));
        let c = fresh("c1", "t1", &url);
        assert!(matches!(
            c.validate(),
            Err(TaskPushConfigValidationError::UrlTooLong(_))
        ));
    }

    #[test]
    fn validate_rejects_missing_task_id() {
        let mut c = fresh("c1", "t1", "https://example.org/h");
        c.task_id.clear();
        assert!(matches!(
            c.validate(),
            Err(TaskPushConfigValidationError::MissingField("task_id"))
        ));
    }

    #[test]
    fn validate_rejects_oversize_token() {
        let mut c = fresh("c1", "t1", "https://example.org/h");
        c.token = Some("a".repeat(MAX_PUSH_TOKEN_BYTES + 1));
        assert!(matches!(
            c.validate(),
            Err(TaskPushConfigValidationError::TokenTooLong(_))
        ));
    }

    #[test]
    fn validate_rejects_too_many_schemes() {
        let mut c = fresh("c1", "t1", "https://example.org/h");
        c.authentication = Some(PushAuthentication {
            schemes: (0..=MAX_PUSH_SCHEMES_PER_CONFIG)
                .map(|i| format!("s{i}"))
                .collect(),
            credentials: None,
        });
        assert!(matches!(
            c.validate(),
            Err(TaskPushConfigValidationError::TooManySchemes(_))
        ));
    }

    #[test]
    fn storage_id_prefixes_task_id() {
        let c = fresh("cfg-1", "task-7", "https://example.org/h");
        assert_eq!(c.storage_id(), "task-7:cfg-1");
    }

    #[test]
    fn debug_redacts_token_and_credentials() {
        let mut c = fresh("c1", "t1", "https://example.org/h");
        c.token = Some("super-secret-bearer".into());
        c.authentication = Some(PushAuthentication {
            schemes: vec!["api-key".into()],
            credentials: Some("never-leak-this".into()),
        });
        let s = format!("{c:?}");
        assert!(!s.contains("super-secret-bearer"));
        assert!(!s.contains("never-leak-this"));
        assert!(s.contains("[REDACTED]"));
    }

    #[test]
    fn dlq_entry_serializes_camelcase_and_redacts_event_in_debug() {
        let entry = PushDeliveryDlqEntry::new(
            "entry-1",
            "cfg-1",
            "task-1",
            "agents",
            "demo",
            "https://h",
            DlqFailureKind::Exhausted,
            "HTTP 503",
            3,
            r#"{"id":"evt"}"#.to_string(),
        );
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["configId"], "cfg-1");
        assert_eq!(json["taskId"], "task-1");
        assert_eq!(json["failureKind"], "exhausted");
        assert_eq!(json["attempts"], 3);
        assert_eq!(json["eventJson"], r#"{"id":"evt"}"#);
        // Debug must NOT print the event JSON or last_error body —
        // both can carry tenant payload data the operator log
        // wasn't authorized to see.
        let dbg = format!("{entry:?}");
        assert!(!dbg.contains(r#""id":"evt""#));
        assert!(dbg.contains("[REDACTED"));
    }
}
