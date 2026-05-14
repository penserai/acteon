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

use crate::bus_task::{Artifact, MAX_METADATA_VALUE_BYTES, TaskStatus, TaskValidationError};

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

/// A2A `TaskStatusUpdateEvent` (spec §4.2.1) — emitted whenever a
/// Task's lifecycle state changes. Phase 2 Task Engine produces one
/// of these on every state transition; subscribers re-frame as a
/// `statusUpdate` field on the A2A `StreamResponse` SSE envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskStatusUpdateEvent {
    /// The task this update describes.
    pub task_id: String,
    /// Conversation/context this task belongs to. Mirrors `Task.contextId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Lifecycle pin at the moment of the update (state + driving
    /// message + timestamp).
    pub status: TaskStatus,
    /// Free-form metadata (trace IDs, source attribution).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TaskStatusUpdateEvent {
    /// Construct an event for the given task.
    #[must_use]
    pub fn new(task_id: impl Into<String>, status: TaskStatus) -> Self {
        Self {
            task_id: task_id.into(),
            context_id: None,
            status,
            metadata: HashMap::new(),
        }
    }

    /// Validate identity fields and the embedded status.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        validate_task_event_id("taskId", &self.task_id)?;
        if let Some(c) = &self.context_id {
            validate_task_event_id("contextId", c)?;
        }
        self.status.validate()?;
        validate_event_metadata(&self.metadata)?;
        Ok(())
    }
}

/// A2A `TaskArtifactUpdateEvent` (spec §4.2.2) — emitted for each
/// chunk of a streamed artifact (or once for a single-shot artifact).
///
/// `append` and `lastChunk` carry the per-delivery semantics that are
/// *not* on the [`Artifact`] itself: whether the new content replaces
/// or appends to the prior content for this `artifactId`, and whether
/// this is the terminal chunk.
///
/// **Acteon extensions (race safety):** A2A spec is silent on
/// out-of-order delivery. `chunkIndex` and `totalChunks` let the
/// Phase 2 Task Engine enforce:
///
/// - chunk indices are non-negative,
/// - no chunks arrive after `lastChunk = true`,
/// - when `totalChunks` is set, every index in `0..totalChunks` is
///   observed before the artifact is closed.
///
/// Both fields are optional so non-streaming producers can omit them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskArtifactUpdateEvent {
    /// The task this artifact belongs to.
    pub task_id: String,
    /// Conversation/context this task belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// The artifact content for this delivery — identity + parts.
    pub artifact: Artifact,
    /// If true, append `artifact.parts` to the existing artifact with
    /// the same `artifactId`; otherwise replace.
    #[serde(default)]
    pub append: bool,
    /// If true, this is the final chunk for the artifact. The Task
    /// Engine will reject subsequent updates for the same
    /// `artifactId`.
    #[serde(default)]
    pub last_chunk: bool,
    /// Monotonic sequence within this artifact's chunk stream.
    /// Acteon extension; see struct-level docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<i64>,
    /// Total chunks expected for this artifact, set on the
    /// `lastChunk = true` envelope. Acteon extension; see
    /// struct-level docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_chunks: Option<i64>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TaskArtifactUpdateEvent {
    /// Construct a single-shot (non-streamed) artifact event —
    /// `append = false`, `last_chunk = true`, no sequencing.
    #[must_use]
    pub fn single_shot(task_id: impl Into<String>, artifact: Artifact) -> Self {
        Self {
            task_id: task_id.into(),
            context_id: None,
            artifact,
            append: false,
            last_chunk: true,
            chunk_index: None,
            total_chunks: None,
            metadata: HashMap::new(),
        }
    }

    /// Construct a chunk-stream event with sequencing metadata.
    #[must_use]
    pub fn chunk(
        task_id: impl Into<String>,
        artifact: Artifact,
        chunk_index: i64,
        last_chunk: bool,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            context_id: None,
            artifact,
            append: chunk_index > 0,
            last_chunk,
            chunk_index: Some(chunk_index),
            total_chunks: None,
            metadata: HashMap::new(),
        }
    }

    /// Validate identity, sequencing invariants, and the embedded
    /// artifact.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        validate_task_event_id("taskId", &self.task_id)?;
        if let Some(c) = &self.context_id {
            validate_task_event_id("contextId", c)?;
        }
        self.artifact.validate()?;
        if let Some(ix) = self.chunk_index
            && ix < 0
        {
            return Err(TaskValidationError::NegativeChunkIndex(ix));
        }
        if let Some(t) = self.total_chunks
            && t <= 0
        {
            return Err(TaskValidationError::InvalidTotalChunks(t));
        }
        // If a chunk advertises `totalChunks`, its own index must fit
        // inside the asserted range — otherwise a consumer that trusts
        // the cap will hang waiting for an index that can never arrive.
        if let (Some(ix), Some(t)) = (self.chunk_index, self.total_chunks)
            && ix >= t
        {
            return Err(TaskValidationError::ChunkIndexOutOfRange {
                index: ix,
                total: t,
            });
        }
        // First chunk in a stream replaces; subsequent chunks append.
        // A `chunk_index = 0` with `append = true` would silently drop
        // any prior accumulated content — reject so callers don't
        // accidentally lose data.
        if matches!(self.chunk_index, Some(0)) && self.append {
            return Err(TaskValidationError::AppendOnFirstChunk);
        }
        validate_event_metadata(&self.metadata)?;
        Ok(())
    }
}

fn validate_task_event_id(field: &'static str, s: &str) -> Result<(), TaskValidationError> {
    if s.is_empty() {
        return Err(TaskValidationError::EmptyId(field));
    }
    if s.len() > 120 {
        return Err(TaskValidationError::IdTooLong(field));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(TaskValidationError::InvalidIdChar {
            field,
            value: s.to_string(),
        });
    }
    Ok(())
}

fn validate_event_metadata(
    map: &HashMap<String, serde_json::Value>,
) -> Result<(), TaskValidationError> {
    for (k, v) in map {
        if k.is_empty() {
            return Err(TaskValidationError::EmptyMetadataKey);
        }
        let encoded = serde_json::to_vec(v).map_err(|_| TaskValidationError::MetadataInvalid)?;
        if encoded.len() > MAX_METADATA_VALUE_BYTES {
            return Err(TaskValidationError::MetadataValueTooLong { key: k.clone() });
        }
    }
    Ok(())
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

    // --- A2A TaskStatusUpdateEvent ---

    use crate::bus_task::{Artifact, Part, TaskState, TaskStatus};

    fn sample_status() -> TaskStatus {
        TaskStatus::new(TaskState::Working, Utc::now())
    }

    #[test]
    fn task_status_event_validates() {
        TaskStatusUpdateEvent::new("task-1", sample_status())
            .validate()
            .unwrap();
    }

    #[test]
    fn task_status_event_rejects_bad_task_id() {
        let mut e = TaskStatusUpdateEvent::new("bad/id", sample_status());
        e.task_id = "bad/id".into();
        assert!(matches!(
            e.validate(),
            Err(TaskValidationError::InvalidIdChar {
                field: "taskId",
                ..
            })
        ));
    }

    #[test]
    fn task_status_event_serializes_camel_case() {
        let mut e = TaskStatusUpdateEvent::new("task-1", sample_status());
        e.context_id = Some("ctx".into());
        let v = serde_json::to_value(&e).unwrap();
        assert!(v.get("taskId").is_some());
        assert!(v.get("contextId").is_some());
        assert!(v.get("status").is_some());
    }

    // --- A2A TaskArtifactUpdateEvent ---

    fn sample_artifact() -> Artifact {
        Artifact::new("art-1", vec![Part::text("hello")])
    }

    #[test]
    fn task_artifact_single_shot_validates() {
        TaskArtifactUpdateEvent::single_shot("task-1", sample_artifact())
            .validate()
            .unwrap();
    }

    #[test]
    fn task_artifact_chunk_constructor_validates() {
        TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 0, false)
            .validate()
            .unwrap();
        TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 1, false)
            .validate()
            .unwrap();
    }

    #[test]
    fn task_artifact_rejects_negative_chunk_index() {
        let mut e = TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 0, false);
        e.chunk_index = Some(-1);
        assert!(matches!(
            e.validate(),
            Err(TaskValidationError::NegativeChunkIndex(-1))
        ));
    }

    #[test]
    fn task_artifact_rejects_non_positive_total_chunks() {
        let mut e = TaskArtifactUpdateEvent::single_shot("task-1", sample_artifact());
        e.total_chunks = Some(0);
        assert!(matches!(
            e.validate(),
            Err(TaskValidationError::InvalidTotalChunks(0))
        ));
    }

    #[test]
    fn task_artifact_rejects_index_outside_total_range() {
        let mut e = TaskArtifactUpdateEvent::single_shot("task-1", sample_artifact());
        e.chunk_index = Some(5);
        e.total_chunks = Some(5);
        assert!(matches!(
            e.validate(),
            Err(TaskValidationError::ChunkIndexOutOfRange { index: 5, total: 5 })
        ));
    }

    #[test]
    fn task_artifact_rejects_append_on_first_chunk() {
        // append=true with chunk_index=0 would silently overwrite the
        // (nonexistent) prior accumulated content — flag it.
        let mut e = TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 0, false);
        e.append = true;
        assert!(matches!(
            e.validate(),
            Err(TaskValidationError::AppendOnFirstChunk)
        ));
    }

    #[test]
    fn task_artifact_chunk_helper_sets_append_correctly() {
        // First chunk: replace mode.
        let first = TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 0, false);
        assert!(!first.append);
        // Subsequent chunks: append.
        let second = TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 1, false);
        assert!(second.append);
    }

    #[test]
    fn task_artifact_event_serializes_camel_case() {
        let e = TaskArtifactUpdateEvent::chunk("task-1", sample_artifact(), 2, true);
        let v = serde_json::to_value(&e).unwrap();
        assert!(v.get("taskId").is_some());
        assert!(v.get("artifact").is_some());
        assert!(v.get("chunkIndex").is_some());
        assert!(v.get("lastChunk").is_some());
    }
}
