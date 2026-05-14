//! A2A protocol native types — `Task` lifecycle, `Message`, `Part`,
//! `Artifact` (Phase 1).
//!
//! Acteon adopts A2A's eight-state task lifecycle verbatim as the
//! canonical primitive for asynchronous, externally-observable work.
//! Narrower internal enums (`ConversationState`, `ToolResultStatus`)
//! continue to govern their own domains; the Task Engine projects
//! between them at the bus boundary.
//!
//! Wire format: these structs serialize to JSON using `camelCase`
//! field names so the same value can ride straight onto an A2A
//! `JSON-RPC` 2.0 wire envelope (`messageId`, `contextId`, `taskId`,
//! etc.) — distinct from the `snake_case` convention the internal bus
//! state-store records use. The state-store representation simply
//! carries the `camelCase` shape; there's no separate "internal" Task
//! type.
//!
//! State machine:
//!
//! ```text
//!   Submitted ──▶ Working ──▶ Completed (terminal)
//!       │           │ │ │ │
//!       │           │ │ │ └──▶ InputRequired ──▶ Working
//!       │           │ │ │                    ──▶ Canceled
//!       │           │ │ │                    ──▶ Failed
//!       │           │ │ └────▶ AuthRequired  ──▶ Working
//!       │           │ │                      ──▶ Canceled
//!       │           │ │                      ──▶ Failed
//!       │           │ └──────▶ Canceled (terminal)
//!       │           └────────▶ Failed (terminal)
//!       ├──────────────────▶ Canceled (terminal)
//!       ├──────────────────▶ Rejected (terminal)
//!       └──────────────────▶ Failed (terminal)
//! ```
//!
//! Phase 1 scope: types, validation, transition rules. The Task
//! Engine that drives state transitions and persists rows lives in
//! `acteon-gateway` (Phase 2).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// Constants & caps
// ---------------------------------------------------------------------

/// Identifier alphabet: `[a-zA-Z0-9._-]`, 1..=120 bytes. Shared with
/// the rest of the bus so IDs land safely in URL paths, state keys,
/// and Kafka headers.
pub const MAX_ID_LEN: usize = 120;

/// Max characters in a `Part::text` field. Matches the existing
/// publish-edge content cap so an oversized text part can't bypass
/// the bus's content limits by riding inside a Task.
pub const MAX_PART_TEXT_BYTES: usize = 512 * 1024; // 512KB

/// Max `base64` length of a `Part::raw` payload. The 1MB cap aligns
/// with the existing bus payload tier; chunked artifacts are the
/// supported pattern for outputs larger than this.
pub const MAX_PART_RAW_BYTES: usize = 1024 * 1024; // 1MB (base64)

/// Max bytes for a serialized `Part::data` JSON value.
pub const MAX_PART_DATA_BYTES: usize = 1024 * 1024; // 1MB

/// Max history length on a [`Task`]. A2A's `historyLength` query
/// parameter is the client-facing way to bound this on the wire; the
/// server keeps a hard ceiling so a misbehaving producer can't grow
/// the row without bound.
pub const MAX_HISTORY_LEN: usize = 1_000;

/// Max number of artifacts on a [`Task`].
pub const MAX_ARTIFACTS_LEN: usize = 256;

/// Max parts inside a single [`Message`] or [`Artifact`].
pub const MAX_PARTS_PER_CONTAINER: usize = 64;

/// Max bytes in a metadata value (single entry, serialized JSON).
pub const MAX_METADATA_VALUE_BYTES: usize = 4096;

/// Max number of `referenceTaskIds` entries on a [`Message`].
pub const MAX_REFERENCE_TASK_IDS: usize = 32;

/// Max number of `extensions` entries on a [`Message`].
pub const MAX_MESSAGE_EXTENSIONS: usize = 32;

// ---------------------------------------------------------------------
// TaskState
// ---------------------------------------------------------------------

/// A2A canonical task lifecycle. Eight states across three categories:
///
/// - **In progress** (non-terminal, non-interrupt): `Submitted`,
///   `Working`.
/// - **Interrupts** (non-terminal, awaiting external action):
///   `InputRequired`, `AuthRequired`.
/// - **Terminal**: `Completed`, `Failed`, `Canceled`, `Rejected`.
///
/// Distinguishing terminal from interrupt matters for SDK consumers
/// writing "is finished" checks — an interrupt is *not* a final state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum TaskState {
    /// Successfully submitted and acknowledged by the server.
    Submitted,
    /// Actively being processed.
    Working,
    /// Finished successfully.
    Completed,
    /// Finished with error.
    Failed,
    /// Canceled before completion.
    Canceled,
    /// Needs additional input from the user/caller (interrupt).
    InputRequired,
    /// Needs authentication (interrupt).
    AuthRequired,
    /// Agent declined to process the task.
    Rejected,
}

impl TaskState {
    /// True for the four terminal states. Once reached, a task makes
    /// no further transitions.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Rejected,
        )
    }

    /// True for the two interrupt states. The task is paused awaiting
    /// an external event (user input / authentication) and may resume.
    #[must_use]
    pub fn is_interrupt(self) -> bool {
        matches!(self, TaskState::InputRequired | TaskState::AuthRequired)
    }

    /// True iff the task is still moving (non-terminal). Note that
    /// interrupt states return `true` here — they're paused, not done.
    #[must_use]
    pub fn is_in_progress(self) -> bool {
        !self.is_terminal()
    }

    /// True iff a transition from `self` to `next` is allowed.
    ///
    /// Allowed graph:
    /// - From `Submitted`: `Working`, `Canceled`, `Failed`, `Rejected`.
    /// - From `Working`: `Completed`, `Failed`, `Canceled`,
    ///   `InputRequired`, `AuthRequired`.
    /// - From `InputRequired` / `AuthRequired`: `Working`, `Canceled`,
    ///   `Failed`.
    /// - From any terminal state: nothing.
    #[must_use]
    pub fn can_transition_to(self, next: TaskState) -> bool {
        use TaskState::{
            AuthRequired, Canceled, Completed, Failed, InputRequired, Rejected, Submitted, Working,
        };
        matches!(
            (self, next),
            (Submitted, Working | Canceled | Failed | Rejected)
                | (Working, Completed | Failed | Canceled | InputRequired | AuthRequired)
                | (InputRequired | AuthRequired, Working | Canceled | Failed)
        )
    }
}

// ---------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------

/// Who sent a [`Message`]. A2A spec uses `ROLE_USER` / `ROLE_AGENT`
/// constants; we mirror the semantics with a typed enum and serialize
/// to the same wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Role {
    /// External caller — the agent's counterpart.
    User,
    /// The Acteon-side agent producing output.
    Agent,
}

// ---------------------------------------------------------------------
// Part
// ---------------------------------------------------------------------

/// A single piece of content inside a [`Message`] or [`Artifact`].
///
/// Per A2A spec §4.1.6 a Part contains **exactly one** of `text`,
/// `raw` (`base64`-encoded bytes), `url` (file reference), or `data`
/// (JSON value). We model this as a struct with all-optional content
/// fields and enforce the exactly-one invariant in [`Part::validate`].
/// A tagged Rust enum would be cleaner internally but wouldn't match
/// the wire shape A2A clients expect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Part {
    /// Inline text payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `Base64`-encoded inline bytes. Stored as `String` (the encoded
    /// form) so the type matches the wire representation directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// External URL the content can be fetched from. Either inline
    /// (`text` / `raw` / `data`) or by reference (`url`), not both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Structured JSON payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub data: Option<serde_json::Value>,
    /// Optional filename (mostly relevant for `raw`/`url` parts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// IANA media type (e.g. `text/plain`, `application/json`,
    /// `application/pdf`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Part {
    /// Construct a text part.
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            text: Some(value.into()),
            media_type: Some("text/plain".to_string()),
            ..Default::default()
        }
    }

    /// Construct a structured-data part.
    #[must_use]
    pub fn data(value: serde_json::Value) -> Self {
        Self {
            data: Some(value),
            media_type: Some("application/json".to_string()),
            ..Default::default()
        }
    }

    /// Construct a URL-reference part.
    #[must_use]
    pub fn url(href: impl Into<String>) -> Self {
        Self {
            url: Some(href.into()),
            ..Default::default()
        }
    }

    /// Construct a raw (`base64`-encoded) bytes part.
    #[must_use]
    pub fn raw_base64(encoded: impl Into<String>) -> Self {
        Self {
            raw: Some(encoded.into()),
            ..Default::default()
        }
    }

    /// Validate the exactly-one-of invariant and bounded sizes.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        let set_count = [
            self.text.is_some(),
            self.raw.is_some(),
            self.url.is_some(),
            self.data.is_some(),
        ]
        .iter()
        .filter(|&&b| b)
        .count();

        if set_count == 0 {
            return Err(TaskValidationError::EmptyPart);
        }
        if set_count > 1 {
            return Err(TaskValidationError::AmbiguousPart);
        }

        if let Some(t) = &self.text
            && t.len() > MAX_PART_TEXT_BYTES
        {
            return Err(TaskValidationError::PartTextTooLong);
        }
        if let Some(r) = &self.raw
            && r.len() > MAX_PART_RAW_BYTES
        {
            return Err(TaskValidationError::PartRawTooLong);
        }
        if let Some(d) = &self.data {
            let encoded =
                serde_json::to_vec(d).map_err(|_| TaskValidationError::PartDataInvalid)?;
            if encoded.len() > MAX_PART_DATA_BYTES {
                return Err(TaskValidationError::PartDataTooLong);
            }
        }
        validate_metadata(&self.metadata)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------

/// A single message in a Task's history. Role-tagged with one or more
/// content [`Part`]s.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Message {
    /// Stable identifier (server- or client-generated UUID). Used for
    /// idempotency on submit; the Task Engine deduplicates by
    /// `messageId`.
    pub message_id: String,
    /// Conversation/context this message belongs to. Maps onto an
    /// Acteon `conversation_id` when the Task is backed by a chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Task this message is associated with. `None` for the initial
    /// message of a `SendMessage` call before a Task has been minted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// User-side vs. agent-side authorship.
    pub role: Role,
    /// One or more content parts. Order is significant.
    pub parts: Vec<Part>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
    /// A2A extension URIs this message participates in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// Other task IDs this message references (e.g. a follow-up that
    /// cites prior tasks). String IDs only — not nested Task objects —
    /// so there's no recursive schema for utoipa to choke on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_task_ids: Vec<String>,
}

impl Message {
    /// Construct a minimal message with a single text part.
    #[must_use]
    pub fn text(message_id: impl Into<String>, role: Role, text: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            context_id: None,
            task_id: None,
            role,
            parts: vec![Part::text(text)],
            metadata: HashMap::new(),
            extensions: Vec::new(),
            reference_task_ids: Vec::new(),
        }
    }

    /// Validate identity, bounded fields, and each part.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        validate_id("messageId", &self.message_id)?;
        if let Some(c) = &self.context_id {
            validate_id("contextId", c)?;
        }
        if let Some(t) = &self.task_id {
            validate_id("taskId", t)?;
        }
        if self.parts.is_empty() {
            return Err(TaskValidationError::MessageHasNoParts);
        }
        if self.parts.len() > MAX_PARTS_PER_CONTAINER {
            return Err(TaskValidationError::TooManyParts);
        }
        for p in &self.parts {
            p.validate()?;
        }
        if self.reference_task_ids.len() > MAX_REFERENCE_TASK_IDS {
            return Err(TaskValidationError::TooManyReferenceTaskIds);
        }
        for r in &self.reference_task_ids {
            validate_id("referenceTaskIds", r)?;
        }
        if self.extensions.len() > MAX_MESSAGE_EXTENSIONS {
            return Err(TaskValidationError::TooManyExtensions);
        }
        validate_metadata(&self.metadata)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Artifact
// ---------------------------------------------------------------------

/// A discrete output produced by the agent, possibly streamed in
/// chunks. `append` and `lastChunk` enable progressive delivery: a
/// stream of artifact-update events with the same `artifactId`, the
/// first carrying `append = false` (replace) and subsequent ones
/// `append = true` (append). The final chunk sets `lastChunk = true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Artifact {
    /// Stable identifier within the parent [`Task`]. Used to stitch
    /// streamed chunks back together.
    pub artifact_id: String,
    /// Optional human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Content parts. For a single-shot artifact this is the full
    /// content; for streamed artifacts each chunk delivers its slice.
    pub parts: Vec<Part>,
    /// True if this update should be appended to the prior content
    /// for this `artifactId`; false (default) means replace.
    #[serde(default)]
    pub append: bool,
    /// True if this is the final chunk for the artifact. Consumers
    /// can mark the artifact as complete once observed.
    #[serde(default)]
    pub last_chunk: bool,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Artifact {
    /// Construct a single-shot artifact with one or more parts.
    #[must_use]
    pub fn new(artifact_id: impl Into<String>, parts: Vec<Part>) -> Self {
        Self {
            artifact_id: artifact_id.into(),
            name: None,
            description: None,
            parts,
            append: false,
            last_chunk: true,
            metadata: HashMap::new(),
        }
    }

    /// Validate identity and bounded fields.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        validate_id("artifactId", &self.artifact_id)?;
        if self.parts.is_empty() {
            return Err(TaskValidationError::ArtifactHasNoParts);
        }
        if self.parts.len() > MAX_PARTS_PER_CONTAINER {
            return Err(TaskValidationError::TooManyParts);
        }
        for p in &self.parts {
            p.validate()?;
        }
        validate_metadata(&self.metadata)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// TaskStatus
// ---------------------------------------------------------------------

/// The lifecycle pin for a [`Task`] — current state, the message that
/// drove the most recent transition (if any), and a timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskStatus {
    /// Current state.
    pub state: TaskState,
    /// Most recent message attached to the status (e.g. the error
    /// message on `Failed`, the prompt on `InputRequired`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// Timestamp of the last state transition.
    pub timestamp: DateTime<Utc>,
}

impl TaskStatus {
    /// Construct a status pin for the given state at the given time.
    #[must_use]
    pub fn new(state: TaskState, timestamp: DateTime<Utc>) -> Self {
        Self {
            state,
            message: None,
            timestamp,
        }
    }

    /// Validate the attached message, if any.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        if let Some(m) = &self.message {
            m.validate()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------

/// An A2A task — the canonical unit of asynchronous work observable
/// to external callers. Lives at `KeyKind::A2aTask` (added in Phase 2)
/// keyed by `(namespace, tenant, task_id)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Task {
    /// Stable task identifier.
    pub id: String,
    /// Conversation/context this task is associated with. Optional in
    /// the A2A spec; populated when the task is bound to a
    /// conversation (which is the default for chain-backed tasks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Current lifecycle pin.
    pub status: TaskStatus,
    /// Outputs produced so far. May be empty until the task reaches a
    /// terminal state (or earlier for streamed tasks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Message history (capped at [`MAX_HISTORY_LEN`]; A2A's
    /// `historyLength` query parameter trims this on read).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Message>,
    /// Free-form task metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
    // --- Acteon-side fields (not in A2A spec) ---
    /// Namespace owning the task.
    pub namespace: String,
    /// Tenant owning the task.
    pub tenant: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last mutation timestamp.
    pub updated_at: DateTime<Utc>,
}

impl Task {
    /// Construct a fresh task in the `Submitted` state.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            context_id: None,
            status: TaskStatus::new(TaskState::Submitted, now),
            artifacts: Vec::new(),
            history: Vec::new(),
            metadata: HashMap::new(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Apply a state transition, capturing the driving message. Rejects
    /// illegal transitions explicitly so the API can surface a 409
    /// rather than silently dropping the change.
    pub fn transition_to(
        &mut self,
        next: TaskState,
        message: Option<Message>,
    ) -> Result<(), TaskValidationError> {
        if !self.status.state.can_transition_to(next) {
            return Err(TaskValidationError::IllegalTransition {
                from: self.status.state,
                to: next,
            });
        }
        if let Some(m) = &message {
            m.validate()?;
        }
        let now = Utc::now();
        self.status = TaskStatus {
            state: next,
            message,
            timestamp: now,
        };
        self.updated_at = now;
        Ok(())
    }

    /// Append a message to the history, enforcing the cap.
    pub fn append_history(&mut self, message: Message) -> Result<(), TaskValidationError> {
        if self.history.len() >= MAX_HISTORY_LEN {
            return Err(TaskValidationError::HistoryFull);
        }
        message.validate()?;
        self.history.push(message);
        self.updated_at = Utc::now();
        Ok(())
    }

    /// Append (or replace) an artifact. If an artifact with the same
    /// `artifactId` already exists and the new one carries
    /// `append = true`, its parts are appended to the existing one;
    /// otherwise the existing artifact is replaced.
    pub fn upsert_artifact(&mut self, artifact: Artifact) -> Result<(), TaskValidationError> {
        artifact.validate()?;
        match self
            .artifacts
            .iter_mut()
            .find(|a| a.artifact_id == artifact.artifact_id)
        {
            Some(existing) if artifact.append => {
                existing.parts.extend(artifact.parts);
                existing.last_chunk = artifact.last_chunk;
                if existing.parts.len() > MAX_PARTS_PER_CONTAINER {
                    return Err(TaskValidationError::TooManyParts);
                }
            }
            Some(existing) => {
                *existing = artifact;
            }
            None => {
                if self.artifacts.len() >= MAX_ARTIFACTS_LEN {
                    return Err(TaskValidationError::TooManyArtifacts);
                }
                self.artifacts.push(artifact);
            }
        }
        self.updated_at = Utc::now();
        Ok(())
    }

    /// Validate identity, status, and all contained messages/artifacts.
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        validate_id("id", &self.id)?;
        validate_fragment("namespace", &self.namespace)?;
        validate_fragment("tenant", &self.tenant)?;
        if let Some(c) = &self.context_id {
            validate_id("contextId", c)?;
        }
        self.status.validate()?;
        if self.history.len() > MAX_HISTORY_LEN {
            return Err(TaskValidationError::HistoryFull);
        }
        for m in &self.history {
            m.validate()?;
        }
        if self.artifacts.len() > MAX_ARTIFACTS_LEN {
            return Err(TaskValidationError::TooManyArtifacts);
        }
        for a in &self.artifacts {
            a.validate()?;
        }
        validate_metadata(&self.metadata)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Validation helpers & errors
// ---------------------------------------------------------------------

fn validate_id(field: &'static str, s: &str) -> Result<(), TaskValidationError> {
    if s.is_empty() {
        return Err(TaskValidationError::EmptyId(field));
    }
    if s.len() > MAX_ID_LEN {
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

fn validate_fragment(field: &'static str, s: &str) -> Result<(), TaskValidationError> {
    if s.is_empty() {
        return Err(TaskValidationError::EmptyFragment(field));
    }
    if s.len() > 80 {
        return Err(TaskValidationError::FragmentTooLong(field));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(TaskValidationError::InvalidFragmentChar {
            field,
            value: s.to_string(),
        });
    }
    Ok(())
}

fn validate_metadata(map: &HashMap<String, serde_json::Value>) -> Result<(), TaskValidationError> {
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

/// Validation failures across the A2A task model.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaskValidationError {
    #[error("{0} must not be empty")]
    EmptyId(&'static str),
    #[error("{0} exceeds 120 characters")]
    IdTooLong(&'static str),
    #[error("{field} '{value}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar { field: &'static str, value: String },
    #[error("{0} must not be empty")]
    EmptyFragment(&'static str),
    #[error("{0} exceeds 80 characters")]
    FragmentTooLong(&'static str),
    #[error("{field} '{value}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidFragmentChar { field: &'static str, value: String },
    #[error("part must set exactly one of text/raw/url/data; none were set")]
    EmptyPart,
    #[error("part must set exactly one of text/raw/url/data; multiple were set")]
    AmbiguousPart,
    #[error("part text exceeds the {MAX_PART_TEXT_BYTES}-byte cap")]
    PartTextTooLong,
    #[error("part raw exceeds the {MAX_PART_RAW_BYTES}-byte cap")]
    PartRawTooLong,
    #[error("part data exceeds the {MAX_PART_DATA_BYTES}-byte cap")]
    PartDataTooLong,
    #[error("part data is not serializable JSON")]
    PartDataInvalid,
    #[error("message must contain at least one part")]
    MessageHasNoParts,
    #[error("artifact must contain at least one part")]
    ArtifactHasNoParts,
    #[error("container exceeds the {MAX_PARTS_PER_CONTAINER}-part cap")]
    TooManyParts,
    #[error("task history exceeds the {MAX_HISTORY_LEN}-message cap")]
    HistoryFull,
    #[error("task artifacts exceed the {MAX_ARTIFACTS_LEN}-entry cap")]
    TooManyArtifacts,
    #[error("message references exceed the {MAX_REFERENCE_TASK_IDS}-id cap")]
    TooManyReferenceTaskIds,
    #[error("message extensions exceed the {MAX_MESSAGE_EXTENSIONS}-entry cap")]
    TooManyExtensions,
    #[error("metadata key must not be empty")]
    EmptyMetadataKey,
    #[error("metadata value for key '{key}' exceeds the {MAX_METADATA_VALUE_BYTES}-byte cap")]
    MetadataValueTooLong { key: String },
    #[error("metadata value is not serializable JSON")]
    MetadataInvalid,
    #[error("illegal transition from {from:?} to {to:?}")]
    IllegalTransition { from: TaskState, to: TaskState },
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- TaskState classification ---

    #[test]
    fn terminal_states_classified() {
        for s in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Canceled,
            TaskState::Rejected,
        ] {
            assert!(s.is_terminal(), "{s:?} should be terminal");
            assert!(!s.is_in_progress(), "{s:?} should not be in_progress");
            assert!(!s.is_interrupt(), "{s:?} should not be interrupt");
        }
    }

    #[test]
    fn interrupt_states_classified() {
        for s in [TaskState::InputRequired, TaskState::AuthRequired] {
            assert!(s.is_interrupt(), "{s:?} should be interrupt");
            assert!(!s.is_terminal(), "{s:?} should not be terminal");
            assert!(s.is_in_progress(), "{s:?} should be in_progress");
        }
    }

    #[test]
    fn in_progress_states_classified() {
        for s in [TaskState::Submitted, TaskState::Working] {
            assert!(s.is_in_progress());
            assert!(!s.is_terminal());
            assert!(!s.is_interrupt());
        }
    }

    // --- Transitions ---

    #[test]
    fn submitted_can_go_to_working() {
        assert!(TaskState::Submitted.can_transition_to(TaskState::Working));
    }

    #[test]
    fn submitted_can_be_rejected_directly() {
        assert!(TaskState::Submitted.can_transition_to(TaskState::Rejected));
    }

    #[test]
    fn submitted_cannot_complete_directly() {
        assert!(!TaskState::Submitted.can_transition_to(TaskState::Completed));
    }

    #[test]
    fn working_can_interrupt() {
        assert!(TaskState::Working.can_transition_to(TaskState::InputRequired));
        assert!(TaskState::Working.can_transition_to(TaskState::AuthRequired));
    }

    #[test]
    fn working_can_complete_fail_cancel() {
        for next in [TaskState::Completed, TaskState::Failed, TaskState::Canceled] {
            assert!(TaskState::Working.can_transition_to(next));
        }
    }

    #[test]
    fn interrupts_resume_to_working() {
        for from in [TaskState::InputRequired, TaskState::AuthRequired] {
            assert!(from.can_transition_to(TaskState::Working));
        }
    }

    #[test]
    fn interrupts_cannot_complete_directly() {
        for from in [TaskState::InputRequired, TaskState::AuthRequired] {
            assert!(!from.can_transition_to(TaskState::Completed));
        }
    }

    #[test]
    fn interrupts_cannot_be_rejected() {
        for from in [TaskState::InputRequired, TaskState::AuthRequired] {
            assert!(!from.can_transition_to(TaskState::Rejected));
        }
    }

    #[test]
    fn terminal_states_have_no_transitions() {
        for from in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Canceled,
            TaskState::Rejected,
        ] {
            for to in [
                TaskState::Submitted,
                TaskState::Working,
                TaskState::Completed,
                TaskState::Failed,
                TaskState::Canceled,
                TaskState::InputRequired,
                TaskState::AuthRequired,
                TaskState::Rejected,
            ] {
                assert!(
                    !from.can_transition_to(to),
                    "{from:?} should be terminal but allowed transition to {to:?}"
                );
            }
        }
    }

    // --- Part ---

    #[test]
    fn part_text_validates() {
        Part::text("hello").validate().unwrap();
    }

    #[test]
    fn part_data_validates() {
        Part::data(json!({"k": "v"})).validate().unwrap();
    }

    #[test]
    fn part_url_validates() {
        Part::url("https://example.com/doc.pdf").validate().unwrap();
    }

    #[test]
    fn part_rejects_empty() {
        let p = Part::default();
        assert_eq!(p.validate(), Err(TaskValidationError::EmptyPart));
    }

    #[test]
    fn part_rejects_ambiguous() {
        let mut p = Part::text("hello");
        p.url = Some("https://example.com".into());
        assert_eq!(p.validate(), Err(TaskValidationError::AmbiguousPart));
    }

    #[test]
    fn part_text_capped() {
        let mut p = Part::text("x");
        p.text = Some("x".repeat(MAX_PART_TEXT_BYTES + 1));
        assert_eq!(p.validate(), Err(TaskValidationError::PartTextTooLong));
    }

    #[test]
    fn part_raw_capped() {
        let mut p = Part::raw_base64("Zg==");
        p.raw = Some("x".repeat(MAX_PART_RAW_BYTES + 1));
        assert_eq!(p.validate(), Err(TaskValidationError::PartRawTooLong));
    }

    // --- Message ---

    #[test]
    fn message_text_validates() {
        Message::text("msg-1", Role::User, "hi").validate().unwrap();
    }

    #[test]
    fn message_rejects_empty_parts() {
        let mut m = Message::text("msg-1", Role::User, "hi");
        m.parts.clear();
        assert_eq!(m.validate(), Err(TaskValidationError::MessageHasNoParts));
    }

    #[test]
    fn message_rejects_too_many_parts() {
        let mut m = Message::text("msg-1", Role::User, "hi");
        for _ in 0..MAX_PARTS_PER_CONTAINER {
            m.parts.push(Part::text("p"));
        }
        assert_eq!(m.validate(), Err(TaskValidationError::TooManyParts));
    }

    #[test]
    fn message_validates_reference_task_ids() {
        let mut m = Message::text("msg-1", Role::User, "hi");
        m.reference_task_ids = vec!["task-1".into(), "bad/id".into()];
        assert!(matches!(
            m.validate(),
            Err(TaskValidationError::InvalidIdChar {
                field: "referenceTaskIds",
                ..
            })
        ));
    }

    #[test]
    fn message_caps_reference_task_ids() {
        let mut m = Message::text("msg-1", Role::User, "hi");
        m.reference_task_ids = (0..=MAX_REFERENCE_TASK_IDS)
            .map(|i| format!("task-{i}"))
            .collect();
        assert_eq!(
            m.validate(),
            Err(TaskValidationError::TooManyReferenceTaskIds)
        );
    }

    // --- Artifact ---

    #[test]
    fn artifact_validates() {
        Artifact::new("art-1", vec![Part::text("done")])
            .validate()
            .unwrap();
    }

    #[test]
    fn artifact_rejects_empty_parts() {
        let a = Artifact::new("art-1", vec![]);
        assert_eq!(a.validate(), Err(TaskValidationError::ArtifactHasNoParts));
    }

    // --- Task transitions ---

    fn sample_task() -> Task {
        Task::new("task-1", "agents", "demo")
    }

    #[test]
    fn task_starts_in_submitted() {
        let t = sample_task();
        assert_eq!(t.status.state, TaskState::Submitted);
        t.validate().unwrap();
    }

    #[test]
    fn task_legal_happy_path() {
        let mut t = sample_task();
        t.transition_to(TaskState::Working, None).unwrap();
        t.transition_to(TaskState::Completed, None).unwrap();
        assert_eq!(t.status.state, TaskState::Completed);
        assert!(t.status.state.is_terminal());
    }

    #[test]
    fn task_interrupt_then_resume() {
        let mut t = sample_task();
        t.transition_to(TaskState::Working, None).unwrap();
        t.transition_to(TaskState::InputRequired, None).unwrap();
        t.transition_to(TaskState::Working, None).unwrap();
        t.transition_to(TaskState::Completed, None).unwrap();
    }

    #[test]
    fn task_rejects_illegal_transition() {
        let mut t = sample_task();
        let err = t.transition_to(TaskState::Completed, None).unwrap_err();
        assert!(matches!(
            err,
            TaskValidationError::IllegalTransition {
                from: TaskState::Submitted,
                to: TaskState::Completed,
            }
        ));
    }

    #[test]
    fn task_cannot_resurrect_after_terminal() {
        let mut t = sample_task();
        t.transition_to(TaskState::Working, None).unwrap();
        t.transition_to(TaskState::Completed, None).unwrap();
        assert!(t.transition_to(TaskState::Working, None).is_err());
    }

    #[test]
    fn task_status_message_validated_on_transition() {
        let mut t = sample_task();
        let mut bad = Message::text("msg-1", Role::Agent, "x");
        bad.parts.clear();
        let err = t.transition_to(TaskState::Working, Some(bad)).unwrap_err();
        assert_eq!(err, TaskValidationError::MessageHasNoParts);
        // State must not have changed.
        assert_eq!(t.status.state, TaskState::Submitted);
    }

    #[test]
    fn task_append_history_caps() {
        let mut t = sample_task();
        // Fill to the cap.
        for i in 0..MAX_HISTORY_LEN {
            t.append_history(Message::text(format!("m-{i}"), Role::User, "x"))
                .unwrap();
        }
        // One more should fail.
        let err = t
            .append_history(Message::text("overflow", Role::User, "x"))
            .unwrap_err();
        assert_eq!(err, TaskValidationError::HistoryFull);
    }

    #[test]
    fn task_upsert_artifact_replaces_when_not_appending() {
        let mut t = sample_task();
        t.upsert_artifact(Artifact::new("art-1", vec![Part::text("first")]))
            .unwrap();
        t.upsert_artifact(Artifact::new("art-1", vec![Part::text("second")]))
            .unwrap();
        assert_eq!(t.artifacts.len(), 1);
        assert_eq!(t.artifacts[0].parts[0].text.as_deref(), Some("second"));
    }

    #[test]
    fn task_upsert_artifact_appends() {
        let mut t = sample_task();
        let mut first = Artifact::new("art-1", vec![Part::text("a")]);
        first.last_chunk = false;
        t.upsert_artifact(first).unwrap();

        let mut second = Artifact::new("art-1", vec![Part::text("b")]);
        second.append = true;
        second.last_chunk = true;
        t.upsert_artifact(second).unwrap();

        assert_eq!(t.artifacts.len(), 1);
        assert_eq!(t.artifacts[0].parts.len(), 2);
        assert!(t.artifacts[0].last_chunk);
    }

    #[test]
    fn task_upsert_artifact_caps_count() {
        let mut t = sample_task();
        for i in 0..MAX_ARTIFACTS_LEN {
            t.upsert_artifact(Artifact::new(format!("art-{i}"), vec![Part::text("x")]))
                .unwrap();
        }
        let err = t
            .upsert_artifact(Artifact::new("overflow", vec![Part::text("x")]))
            .unwrap_err();
        assert_eq!(err, TaskValidationError::TooManyArtifacts);
    }

    #[test]
    fn task_validate_rejects_bad_namespace() {
        let mut t = sample_task();
        t.namespace = "bad/ns".into();
        assert!(matches!(
            t.validate(),
            Err(TaskValidationError::InvalidFragmentChar {
                field: "namespace",
                ..
            })
        ));
    }

    // --- Wire format ---

    #[test]
    fn task_serializes_camel_case() {
        let mut t = sample_task();
        t.context_id = Some("ctx-1".into());
        let v = serde_json::to_value(&t).unwrap();
        // Top-level Acteon and A2A fields are camelCase.
        assert!(v.get("contextId").is_some(), "contextId missing: {v}");
        assert!(v.get("createdAt").is_some(), "createdAt missing: {v}");
        assert!(v.get("updatedAt").is_some(), "updatedAt missing: {v}");
        // status nested object is also camelCase.
        let status = v.get("status").unwrap();
        assert!(status.get("state").is_some());
        assert!(status.get("timestamp").is_some());
    }

    #[test]
    fn message_serializes_camel_case() {
        let mut m = Message::text("msg-1", Role::Agent, "hi");
        m.context_id = Some("ctx".into());
        m.task_id = Some("task".into());
        m.reference_task_ids = vec!["t-1".into()];
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("messageId").is_some());
        assert!(v.get("contextId").is_some());
        assert!(v.get("taskId").is_some());
        assert!(v.get("referenceTaskIds").is_some());
    }

    #[test]
    fn role_serializes_snake_case() {
        assert_eq!(serde_json::to_value(Role::User).unwrap(), json!("user"));
        assert_eq!(serde_json::to_value(Role::Agent).unwrap(), json!("agent"));
    }

    #[test]
    fn task_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(TaskState::InputRequired).unwrap(),
            json!("input_required"),
        );
        assert_eq!(
            serde_json::to_value(TaskState::AuthRequired).unwrap(),
            json!("auth_required"),
        );
    }

    #[test]
    fn task_roundtrip_serde() {
        let mut t = sample_task();
        t.transition_to(TaskState::Working, None).unwrap();
        t.append_history(Message::text("msg-1", Role::User, "hi"))
            .unwrap();
        t.upsert_artifact(Artifact::new("art-1", vec![Part::data(json!({"k": 1}))]))
            .unwrap();
        let j = serde_json::to_string(&t).unwrap();
        let back: Task = serde_json::from_str(&j).unwrap();
        assert_eq!(back.id, t.id);
        assert_eq!(back.status.state, TaskState::Working);
        assert_eq!(back.history.len(), 1);
        assert_eq!(back.artifacts.len(), 1);
    }
}
