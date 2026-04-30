//! Bus streaming envelopes — `StreamChunk` / `StreamEnd` (Phase 6b).
//!
//! Streaming is the natural protocol for LLM token output, partial
//! tool results, progressive search hits, and any other "produce
//! incremental, signal completion" pattern. Acteon ships streaming as
//! a typed convention over the existing conversation events topic
//! (same architectural pattern as Phase 6a tool envelopes): each
//! chunk is its own Kafka record carrying `(stream_id, chunk_seq)`,
//! and a terminal `StreamEnd` record marks the stream complete.
//!
//! Subscribers route on `acteon.envelope.kind` and `acteon.stream.id`
//! headers — no payload deserialization until they've matched the
//! stream they care about.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Terminal status for a stream. `Complete` is the happy path —
/// every chunk made it. `Aborted` is producer-side cancellation
/// (the agent gave up cleanly). `Error` carries the reason the
/// stream failed mid-flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum StreamEndStatus {
    /// All chunks delivered successfully.
    Complete,
    /// Producer canceled the stream cleanly. Distinct from `Error`
    /// so consumers can retry vs. surface differently.
    Aborted,
    /// Stream failed mid-flight. `error_message` carries detail.
    Error,
}

/// A single chunk in a stream. Each chunk lives in its own bus
/// message; consumers re-stitch them into a contiguous stream by
/// `stream_id` and `chunk_seq`. The `body` is opaque to the bus —
/// LLM tokens, partial JSON, raw bytes encoded as a string, whatever
/// the producer and consumer agree on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StreamChunk {
    /// Stable identifier for the stream this chunk belongs to.
    /// Stamped as `acteon.stream.id` on the underlying Kafka message
    /// so subscribers can header-filter.
    pub stream_id: String,
    /// Monotonic sequence number within the stream. The bus does not
    /// enforce ordering on its own — consumers may sort by this
    /// field when reassembling a partitioned scan. (For
    /// single-conversation streams keyed by `conversation_id`,
    /// Kafka's per-partition ordering already gives FIFO; `chunk_seq`
    /// is mostly diagnostic.)
    pub chunk_seq: i64,
    /// Chunk payload. Free-form JSON to match the rest of the bus —
    /// callers convention-encode tokens, partial structured results,
    /// or whatever shape they need.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub body: serde_json::Value,
    /// `agent_id` of the producer. Mirrors the conversation-message
    /// `sender` on the underlying record; carried on the envelope so
    /// audit and replay see it without parsing the conversation
    /// header.
    #[serde(default)]
    pub sender: Option<String>,
    /// Free-form metadata (e.g. trace IDs). Bounded by the publish
    /// path's label caps.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// When the chunk envelope was constructed. Distinct from the
    /// broker-stamped `produced_at`.
    pub created_at: DateTime<Utc>,
}

impl StreamChunk {
    /// Construct a fresh chunk envelope.
    #[must_use]
    pub fn new(stream_id: impl Into<String>, chunk_seq: i64, body: serde_json::Value) -> Self {
        Self {
            stream_id: stream_id.into(),
            chunk_seq,
            body,
            sender: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Validate identity fields. Reuses the same alphabet rules as
    /// the rest of the bus envelopes — `[a-zA-Z0-9._-]`, max 120
    /// chars on `stream_id` / `sender`. Rejects negative
    /// `chunk_seq` so consumers can sort with confidence that
    /// non-negative is the only reachable shape.
    pub fn validate(&self) -> Result<(), StreamEnvelopeValidationError> {
        validate_id_field("stream_id", &self.stream_id)?;
        if self.chunk_seq < 0 {
            return Err(StreamEnvelopeValidationError::NegativeChunkSeq(
                self.chunk_seq,
            ));
        }
        if let Some(s) = &self.sender {
            validate_id_field("sender", s)?;
        }
        Ok(())
    }
}

/// Terminal record for a stream. Marks the stream complete (or not)
/// and carries an optional error message. Consumers stop reading
/// once they see this for their `stream_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StreamEnd {
    /// The stream this terminal marker closes.
    pub stream_id: String,
    /// Sequence number of the terminal record. Conventionally
    /// `last_chunk_seq + 1`, though the bus does not enforce this —
    /// a producer that knows its tail can use any non-negative
    /// number for diagnostic clarity.
    pub chunk_seq: i64,
    /// How the stream ended.
    pub status: StreamEndStatus,
    /// Human-readable detail. Required for `Error`; optional
    /// otherwise. Always capped at 4096 bytes regardless of status —
    /// a malicious caller could otherwise ship a multi-MB string
    /// alongside `Complete` and bypass the limit.
    #[serde(default)]
    pub error_message: Option<String>,
    /// `agent_id` of the producer. Same as on `StreamChunk`.
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

impl StreamEnd {
    /// Construct a terminal marker for a successful stream.
    #[must_use]
    pub fn complete(stream_id: impl Into<String>, chunk_seq: i64) -> Self {
        Self {
            stream_id: stream_id.into(),
            chunk_seq,
            status: StreamEndStatus::Complete,
            error_message: None,
            sender: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Construct a terminal marker for a failed stream.
    #[must_use]
    pub fn error(stream_id: impl Into<String>, chunk_seq: i64, message: impl Into<String>) -> Self {
        Self {
            stream_id: stream_id.into(),
            chunk_seq,
            status: StreamEndStatus::Error,
            error_message: Some(message.into()),
            sender: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Validate identity fields and the bounded `error_message`.
    pub fn validate(&self) -> Result<(), StreamEnvelopeValidationError> {
        validate_id_field("stream_id", &self.stream_id)?;
        if self.chunk_seq < 0 {
            return Err(StreamEnvelopeValidationError::NegativeChunkSeq(
                self.chunk_seq,
            ));
        }
        if let Some(s) = &self.sender {
            validate_id_field("sender", s)?;
        }
        // Cap unconditionally — same reasoning as `ToolResult`. A
        // caller could otherwise ship a megabyte of `error_message`
        // alongside `status: complete` to dodge the limit.
        if let Some(m) = &self.error_message
            && m.len() > 4096
        {
            return Err(StreamEnvelopeValidationError::ErrorMessageTooLong);
        }
        Ok(())
    }
}

/// Shared id-field validation used across `StreamChunk` and
/// `StreamEnd`. Mirrors the rule applied to `ToolCall` / `Agent` /
/// `Conversation` ids: alphanumeric plus `[._-]`, 1..=120 bytes.
pub fn validate_id_field(field: &str, s: &str) -> Result<(), StreamEnvelopeValidationError> {
    if s.is_empty() {
        return Err(StreamEnvelopeValidationError::EmptyId(field.to_string()));
    }
    if s.len() > 120 {
        return Err(StreamEnvelopeValidationError::IdTooLong(field.to_string()));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(StreamEnvelopeValidationError::InvalidIdChar {
            field: field.to_string(),
            value: s.to_string(),
        });
    }
    Ok(())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StreamEnvelopeValidationError {
    #[error("{0} must not be empty")]
    EmptyId(String),
    #[error("{0} exceeds 120 characters")]
    IdTooLong(String),
    #[error("{field} '{value}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar { field: String, value: String },
    #[error("chunk_seq must be non-negative (got {0})")]
    NegativeChunkSeq(i64),
    #[error("error_message exceeds 4096 characters")]
    ErrorMessageTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chunk_basic_validate() {
        let mut c = StreamChunk::new("stream-1", 0, json!({"token": "hello"}));
        c.sender = Some("planner-1".into());
        c.validate().unwrap();
    }

    #[test]
    fn chunk_rejects_negative_seq() {
        let c = StreamChunk::new("s", -1, json!({}));
        assert_eq!(
            c.validate(),
            Err(StreamEnvelopeValidationError::NegativeChunkSeq(-1))
        );
    }

    #[test]
    fn chunk_rejects_invalid_id() {
        let c = StreamChunk::new("a/b", 0, json!({}));
        assert!(matches!(
            c.validate(),
            Err(StreamEnvelopeValidationError::InvalidIdChar { .. })
        ));
    }

    #[test]
    fn end_complete_validate() {
        let e = StreamEnd::complete("stream-1", 42);
        e.validate().unwrap();
        assert_eq!(e.status, StreamEndStatus::Complete);
        assert!(e.error_message.is_none());
    }

    #[test]
    fn end_error_validate() {
        let e = StreamEnd::error("stream-1", 7, "upstream gave up");
        e.validate().unwrap();
        assert_eq!(e.status, StreamEndStatus::Error);
    }

    #[test]
    fn end_caps_error_message_unconditionally() {
        // The cap fires even on `Complete` — a caller that ships a
        // megabyte of `error_message` with status=complete would
        // otherwise bypass the limit.
        let mut e = StreamEnd::complete("s", 0);
        e.error_message = Some("x".repeat(5000));
        assert_eq!(
            e.validate(),
            Err(StreamEnvelopeValidationError::ErrorMessageTooLong)
        );
    }

    #[test]
    fn roundtrip_serde_chunk() {
        let mut c = StreamChunk::new("s-1", 5, json!({"k": "v"}));
        c.sender = Some("a-1".into());
        c.metadata.insert("trace".into(), "abc".into());
        let j = serde_json::to_string(&c).unwrap();
        let back: StreamChunk = serde_json::from_str(&j).unwrap();
        assert_eq!(back.stream_id, c.stream_id);
        assert_eq!(back.chunk_seq, c.chunk_seq);
        assert_eq!(back.metadata.get("trace"), Some(&"abc".into()));
    }

    #[test]
    fn roundtrip_serde_end_aborted() {
        let mut e = StreamEnd::complete("s", 10);
        e.status = StreamEndStatus::Aborted;
        e.error_message = Some("user canceled".into());
        let j = serde_json::to_string(&e).unwrap();
        let back: StreamEnd = serde_json::from_str(&j).unwrap();
        assert_eq!(back.status, StreamEndStatus::Aborted);
        assert_eq!(back.error_message.as_deref(), Some("user canceled"));
    }
}
