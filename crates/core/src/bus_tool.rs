//! Bus tool-call envelopes — `ToolCall` / `ToolResult` types that
//! ride on top of conversation messages (Phase 6a).
//!
//! Tool-calling is the dominant agent-to-agent protocol. Rather than
//! building a parallel Kafka pipeline, Acteon layers typed envelopes
//! over the existing conversation events topic:
//!
//! - A `ToolCall` is a conversation message with envelope kind
//!   `"tool_call"`, the structured `ToolCall` body as the payload,
//!   and `acteon.tool.call_id`, `acteon.envelope.kind`, optionally
//!   `acteon.correlation_id` and `acteon.reply_to` as
//!   server-stamped headers.
//! - A `ToolResult` is a conversation message with envelope kind
//!   `"tool_result"` and the matching `acteon.tool.call_id`.
//!
//! Subscribers can route tool messages locally without parsing the
//! payload by matching on the headers; the bus's audit and replay
//! machinery already governs the underlying transport.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Outcome of a tool invocation. A typed status keeps callers from
/// having to inspect free-form fields to know if the call succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ToolResultStatus {
    /// The tool ran and produced a (possibly empty) output.
    Ok,
    /// The tool ran but returned a structured error. `error_message`
    /// on the [`ToolResult`] carries the human-readable detail.
    Error,
    /// The tool was canceled before producing a result. Distinct from
    /// `Error` so callers can retry or escalate selectively.
    Canceled,
}

/// A request to invoke a tool, addressed to an agent participant in a
/// conversation. Wrapped in a conversation message at the bus layer;
/// the structured fields here become the payload, and the bus stamps
/// routing headers separately so subscribers don't need to parse the
/// JSON to filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolCall {
    /// Stable identifier for this call. Replies cite it via
    /// [`ToolResult::call_id`]; the bus stamps it as
    /// `acteon.tool.call_id` so a result-fetcher can filter without
    /// scanning payloads.
    pub call_id: String,
    /// Tool name (e.g. `"calendar.list_events"`). Free-form to the
    /// bus; the receiving agent dispatches on it.
    pub tool: String,
    /// Tool arguments as a JSON object. Tools define their own schema
    /// (Phase 3 schema-bound topics also apply here transparently —
    /// the conversation events topic can carry a schema binding).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub arguments: serde_json::Value,
    /// Operator-supplied correlation token. Same value appears on the
    /// matching [`ToolResult`] so a request and its response thread
    /// together regardless of intermediate hops. Stamped as
    /// `acteon.correlation_id`.
    #[serde(default)]
    pub correlation_id: Option<String>,
    /// Where the responder should send the [`ToolResult`]. Empty
    /// means "same conversation"; set this when the request and
    /// response should land in different threads (e.g. a dispatcher
    /// agent that fans out tool calls but collects results in a
    /// dedicated thread). Stamped as `acteon.reply_to`.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// `agent_id` of the caller. The bus also stamps this on the
    /// underlying conversation message via `acteon.conversation.sender`,
    /// but carrying it on the envelope makes the audit story
    /// payload-only.
    #[serde(default)]
    pub sender: Option<String>,
    /// Free-form metadata an agent can attach to the call (e.g.
    /// trace IDs, cost budget). Bounded by the publish path's header
    /// caps — see `validate_user_labels` on the server.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// When the envelope was constructed. Distinct from the bus's
    /// `produced_at` (which is broker-stamped) so agent-side timing
    /// signals survive the queue.
    pub created_at: DateTime<Utc>,
}

impl ToolCall {
    /// Construct a tool-call envelope with sensible defaults.
    #[must_use]
    pub fn new(
        call_id: impl Into<String>,
        tool: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool: tool.into(),
            arguments,
            correlation_id: None,
            reply_to: None,
            sender: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Validate identity fields. The same alphabet as conversation IDs
    /// — `[a-zA-Z0-9._-]` — keeps these safe in URLs and Kafka headers.
    pub fn validate(&self) -> Result<(), ToolEnvelopeValidationError> {
        Self::validate_id_field("call_id", &self.call_id)?;
        if self.tool.is_empty() {
            return Err(ToolEnvelopeValidationError::EmptyTool);
        }
        if self.tool.len() > 200 {
            return Err(ToolEnvelopeValidationError::ToolTooLong);
        }
        if let Some(c) = &self.correlation_id {
            Self::validate_id_field("correlation_id", c)?;
        }
        if let Some(s) = &self.sender {
            Self::validate_id_field("sender", s)?;
        }
        if let Some(r) = &self.reply_to
            && r.is_empty()
        {
            return Err(ToolEnvelopeValidationError::EmptyReplyTo);
        }
        Ok(())
    }

    /// Shared id-character rule used by `call_id`, `correlation_id`,
    /// and `sender`.
    pub fn validate_id_field(field: &str, s: &str) -> Result<(), ToolEnvelopeValidationError> {
        if s.is_empty() {
            return Err(ToolEnvelopeValidationError::EmptyId(field.to_string()));
        }
        if s.len() > 120 {
            return Err(ToolEnvelopeValidationError::IdTooLong(field.to_string()));
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(ToolEnvelopeValidationError::InvalidIdChar {
                field: field.to_string(),
                value: s.to_string(),
            });
        }
        Ok(())
    }
}

/// Response to a [`ToolCall`]. Carries the same `call_id` so consumers
/// can match a result against the request that triggered it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolResult {
    /// The originating call's `call_id`. Stamped as
    /// `acteon.tool.call_id` on the resulting bus message so a
    /// result-fetcher can filter by header.
    pub call_id: String,
    /// Outcome of the call. See [`ToolResultStatus`].
    pub status: ToolResultStatus,
    /// Tool output payload. Convention: `Ok` results put the structured
    /// return value here; `Error` and `Canceled` results put a
    /// machine-readable error blob if applicable. Tools define the
    /// shape.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub output: serde_json::Value,
    /// Human-readable error message, set on `Error` and `Canceled`.
    #[serde(default)]
    pub error_message: Option<String>,
    /// Mirrors [`ToolCall::correlation_id`]. Stamped as
    /// `acteon.correlation_id`.
    #[serde(default)]
    pub correlation_id: Option<String>,
    /// `agent_id` of the agent that produced the result.
    #[serde(default)]
    pub sender: Option<String>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// When the result envelope was constructed.
    pub created_at: DateTime<Utc>,
}

impl ToolResult {
    /// Construct a successful result.
    #[must_use]
    pub fn ok(call_id: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            status: ToolResultStatus::Ok,
            output,
            error_message: None,
            correlation_id: None,
            sender: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Construct an error result.
    #[must_use]
    pub fn error(call_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            status: ToolResultStatus::Error,
            output: serde_json::Value::Null,
            error_message: Some(message.into()),
            correlation_id: None,
            sender: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Validate identity fields.
    pub fn validate(&self) -> Result<(), ToolEnvelopeValidationError> {
        ToolCall::validate_id_field("call_id", &self.call_id)?;
        if let Some(c) = &self.correlation_id {
            ToolCall::validate_id_field("correlation_id", c)?;
        }
        if let Some(s) = &self.sender {
            ToolCall::validate_id_field("sender", s)?;
        }
        match self.status {
            ToolResultStatus::Error | ToolResultStatus::Canceled => {
                // We don't *require* an error message on canceled
                // results — sometimes a cancel is just a cancel —
                // but if one is supplied, cap its length so a
                // misbehaving agent can't blow up the publish-edge
                // header budget on the conversation message.
                if let Some(m) = &self.error_message
                    && m.len() > 4096
                {
                    return Err(ToolEnvelopeValidationError::ErrorMessageTooLong);
                }
            }
            ToolResultStatus::Ok => {}
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ToolEnvelopeValidationError {
    #[error("{0} must not be empty")]
    EmptyId(String),
    #[error("{0} exceeds 120 characters")]
    IdTooLong(String),
    #[error("{field} '{value}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar { field: String, value: String },
    #[error("tool name must not be empty")]
    EmptyTool,
    #[error("tool name exceeds 200 characters")]
    ToolTooLong,
    #[error("reply_to must not be empty when set")]
    EmptyReplyTo,
    #[error("error_message exceeds 4096 characters")]
    ErrorMessageTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_call_basic_validate() {
        let mut c = ToolCall::new("call-1", "calendar.list", json!({"day": "today"}));
        c.sender = Some("planner-1".into());
        c.correlation_id = Some("trace-42".into());
        c.validate().unwrap();
    }

    #[test]
    fn tool_call_rejects_empty_call_id() {
        let c = ToolCall::new("", "x", json!({}));
        let err = c.validate().unwrap_err();
        assert!(matches!(err, ToolEnvelopeValidationError::EmptyId(f) if f == "call_id"));
    }

    #[test]
    fn tool_call_rejects_empty_tool() {
        let c = ToolCall::new("c", "", json!({}));
        assert_eq!(c.validate(), Err(ToolEnvelopeValidationError::EmptyTool));
    }

    #[test]
    fn tool_call_rejects_invalid_id_char() {
        let c = ToolCall::new("a/b", "tool", json!({}));
        assert!(matches!(
            c.validate(),
            Err(ToolEnvelopeValidationError::InvalidIdChar { .. })
        ));
    }

    #[test]
    fn tool_result_ok_validate() {
        let r = ToolResult::ok("call-1", json!({"events": []}));
        r.validate().unwrap();
        assert_eq!(r.status, ToolResultStatus::Ok);
        assert!(r.error_message.is_none());
    }

    #[test]
    fn tool_result_error_validate() {
        let r = ToolResult::error("call-1", "calendar API timed out");
        r.validate().unwrap();
        assert_eq!(r.status, ToolResultStatus::Error);
    }

    #[test]
    fn tool_result_rejects_oversize_error_message() {
        let mut r = ToolResult::error("call-1", "x");
        r.error_message = Some("y".repeat(5000));
        assert_eq!(
            r.validate(),
            Err(ToolEnvelopeValidationError::ErrorMessageTooLong)
        );
    }

    #[test]
    fn roundtrip_serde_tool_call() {
        let mut c = ToolCall::new("c-1", "tool.x", json!({"k": "v"}));
        c.correlation_id = Some("trace".into());
        c.sender = Some("a-1".into());
        c.metadata.insert("budget".into(), "1.5s".into());
        let j = serde_json::to_string(&c).unwrap();
        let back: ToolCall = serde_json::from_str(&j).unwrap();
        assert_eq!(back.call_id, c.call_id);
        assert_eq!(back.tool, c.tool);
        assert_eq!(back.metadata.get("budget"), Some(&"1.5s".into()));
    }

    #[test]
    fn roundtrip_serde_tool_result_canceled() {
        let mut r = ToolResult::ok("c", json!(null));
        r.status = ToolResultStatus::Canceled;
        r.error_message = Some("user aborted".into());
        let j = serde_json::to_string(&r).unwrap();
        let back: ToolResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.status, ToolResultStatus::Canceled);
        assert_eq!(back.error_message.as_deref(), Some("user aborted"));
    }
}
