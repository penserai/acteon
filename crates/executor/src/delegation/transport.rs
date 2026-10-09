//! Durable outbound A2A submission through one exact operator-qualified binding.
//!
//! The journal is stored in Acteon's configured [`StateStore`]. A durable intent
//! precedes the external call. Ambiguous delivery is never converted into a
//! rejection or an automatic resend. Explicit replay is available only when the
//! installed adapter qualifies identical-message submission as idempotent.

use super::{ApprovedPeerBinding, ApprovedPeerRegistry, PeerDiscoveryError};
use acteon_core::{
    ExecutionContextReference, TASK_CHALLENGE_ID_METADATA_KEY, Task, TaskMessage, TaskRole,
    TaskState,
};
use acteon_crypto::PayloadEncryptor;
use acteon_governance::{
    AuthorityCoordinator, context::VerifiedExecutionContext, permit::PermitReference,
};
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};
use acteon_time::Clock;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

pub const PEER_SEND_KIND: &str = "governed_peer_send";
pub const PEER_CANCEL_KIND: &str = "governed_peer_cancel";
pub const PEER_CONTINUATION_KIND: &str = "governed_peer_continuation";
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const PEER_SEND_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerSubmissionCapability {
    #[default]
    AtMostOnce,
    VerifiedIdempotent,
}

#[derive(Debug, Clone)]
pub enum PeerSendDisposition {
    Accepted {
        task: Box<Task>,
        source_context: ExecutionContextReference,
        progress_cursor: Option<String>,
    },
    Rejected {
        code: String,
    },
    Uncertain,
}

/// Result of one remote cancellation delivery. A restriction acknowledges
/// that future starts are fenced without claiming an in-flight effect stopped.
/// Transport failures and unverified responses remain `Uncertain`.
#[derive(Debug, Clone)]
pub enum PeerCancelDisposition {
    Unsupported,
    Rejected { code: String },
    Uncertain,
    Restricted { task: Box<Task> },
    Final { task: Box<Task> },
}

/// Result of delivering one response to an exact remote `InputRequired`
/// challenge. A transport error or response that cannot be bound to the
/// accepted task remains uncertain.
#[derive(Debug, Clone)]
pub enum PeerContinuationDisposition {
    Rejected {
        code: String,
    },
    Uncertain,
    Accepted {
        task: Box<Task>,
        progress_cursor: String,
    },
}

pub struct PeerSendRequest<'a> {
    pub endpoint: &'a str,
    pub transport: &'a str,
    pub parent: &'a ExecutionContextReference,
    pub permits: &'a [PermitReference],
    pub message: &'a TaskMessage,
}

/// Host-owned remote observation. The source context and endpoint come from
/// the durable accepted send record, never from a model-facing request.
pub struct PeerTaskRequest<'a> {
    pub endpoint: &'a str,
    pub transport: &'a str,
    pub source_context: &'a ExecutionContextReference,
    pub task_id: &'a str,
    /// Opaque validator last issued by the target for this exact task.
    pub progress_cursor: Option<&'a str>,
}

/// Host-owned continuation request. Every routing and authority-bearing field
/// comes from the accepted send journal. `message` is normalized by the
/// transport to the exact task, context, and active challenge before this call.
pub struct PeerContinuationRequest<'a> {
    pub endpoint: &'a str,
    pub transport: &'a str,
    pub source_context: &'a ExecutionContextReference,
    pub task_id: &'a str,
    pub challenge_id: &'a str,
    pub progress_cursor: Option<&'a str>,
    pub message: &'a TaskMessage,
}

/// Caller content plus the opaque durable submission selector. Authority and
/// remote routing remain host-owned inputs to [`DurablePeerTransport`].
pub struct PeerContinuationInput<'a> {
    pub submission_id: Uuid,
    pub response: &'a TaskMessage,
}

/// Result of a conditional remote task observation.
#[derive(Debug, Clone)]
pub enum PeerTaskObservation {
    /// The target confirms that the supplied cursor still names its current
    /// authoritative snapshot.
    Unchanged { progress_cursor: String },
    /// The target returned a task snapshot and, when supported, the opaque
    /// cursor that identifies it.
    Updated {
        task: Box<Task>,
        progress_cursor: Option<String>,
    },
}

#[async_trait]
pub trait PeerTransportAdapter: Send + Sync {
    fn revision(&self) -> &str;
    fn binding_digest(&self) -> &str;
    fn submission_capability(&self) -> PeerSubmissionCapability;
    async fn send(
        &self,
        request: PeerSendRequest<'_>,
    ) -> Result<PeerSendDisposition, PeerTransportError>;
    async fn observe_task(
        &self,
        _request: PeerTaskRequest<'_>,
    ) -> Result<PeerTaskObservation, PeerTransportError> {
        Err(PeerTransportError::Unavailable)
    }
    async fn cancel_task(
        &self,
        _request: PeerTaskRequest<'_>,
    ) -> Result<PeerCancelDisposition, PeerTransportError> {
        Ok(PeerCancelDisposition::Unsupported)
    }
    async fn continue_task(
        &self,
        _request: PeerContinuationRequest<'_>,
    ) -> Result<PeerContinuationDisposition, PeerTransportError> {
        Err(PeerTransportError::Unavailable)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerTransportError {
    #[error("invalid qualified peer submission")]
    Invalid,
    #[error("peer submission conflicts with durable intent")]
    Conflict,
    #[error("peer submission authority was refused")]
    Refused,
    #[error("peer submission storage or adapter unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum SendState {
    Registered,
    Delivering {
        claim_id: Uuid,
    },
    Accepted {
        task: Box<Task>,
        source_context: ExecutionContextReference,
        #[serde(default)]
        progress_cursor: Option<String>,
    },
    Rejected {
        code: String,
    },
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendRecord {
    schema: u32,
    submission_id: Uuid,
    binding_digest: String,
    adapter_revision: String,
    capability: PeerSubmissionCapability,
    endpoint: String,
    transport: String,
    parent: ExecutionContextReference,
    permits: Vec<PermitReference>,
    message: TaskMessage,
    message_digest: String,
    created_at_ms: i64,
    state: SendState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerSendStatus {
    Uncertain,
    Accepted {
        task: Box<Task>,
        source_context: ExecutionContextReference,
    },
    Rejected {
        code: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerSendReceipt {
    pub submission_id: Uuid,
    pub status: PeerSendStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum CancelState {
    Registered,
    Delivering { claim_id: Uuid },
    Unsupported,
    Rejected { code: String },
    Uncertain,
    Restricted { task: Box<Task> },
    Reconciled { task: Box<Task> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelRecord {
    schema: u32,
    submission_id: Uuid,
    cancellation_id: Uuid,
    binding_digest: String,
    adapter_revision: String,
    endpoint: String,
    transport: String,
    source_context: ExecutionContextReference,
    task_id: String,
    requested_at_ms: i64,
    state: CancelState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerCancelStatus {
    Unsupported,
    Rejected { code: String },
    Uncertain,
    Restricted { task: Box<Task> },
    Reconciled { task: Box<Task> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerCancelReceipt {
    pub submission_id: Uuid,
    pub cancellation_id: Uuid,
    pub status: PeerCancelStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ContinuationState {
    Registered,
    Delivering {
        claim_id: Uuid,
    },
    Rejected {
        code: String,
    },
    Uncertain,
    Accepted {
        task: Box<Task>,
        progress_cursor: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationRecord {
    schema: u32,
    submission_id: Uuid,
    continuation_id: Uuid,
    binding_digest: String,
    adapter_revision: String,
    endpoint: String,
    transport: String,
    source_context: ExecutionContextReference,
    task_id: String,
    challenge_id: String,
    prior_task: Box<Task>,
    prior_progress_cursor: Option<String>,
    message: TaskMessage,
    message_digest: String,
    requested_at_ms: i64,
    state: ContinuationState,
}

struct AuthorizedContinuation<'a> {
    binding: &'a ApprovedPeerBinding,
    reference: ExecutionContextReference,
    send_key: StateKey,
    send: SendRecord,
    input: PeerContinuationInput<'a>,
}

struct ContinuedTaskProjection<'a> {
    key: &'a StateKey,
    binding: &'a ApprovedPeerBinding,
    parent: &'a ExecutionContextReference,
    continuation: &'a ContinuationRecord,
    continued_task: &'a Task,
    progress_cursor: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerContinuationStatus {
    Rejected {
        code: String,
    },
    Uncertain,
    Accepted {
        task: Box<Task>,
        progress_cursor: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerContinuationReceipt {
    pub submission_id: Uuid,
    pub continuation_id: Uuid,
    pub status: PeerContinuationStatus,
}

pub struct PeerTransportDependencies {
    pub state: Arc<dyn StateStore>,
    pub coordinator: AuthorityCoordinator,
    pub clock: Arc<dyn Clock>,
    pub encryptor: Option<Arc<PayloadEncryptor>>,
}

pub struct DurablePeerTransport {
    dependencies: PeerTransportDependencies,
    adapter: Arc<dyn PeerTransportAdapter>,
    timeout: Duration,
}

fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}

fn valid_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_progress_cursor(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn message_digest(message: &TaskMessage) -> Result<String, PeerTransportError> {
    let value = serde_json::to_value(message).map_err(|_| PeerTransportError::Invalid)?;
    let bytes = crate::plan::canonical_bytes(&value).map_err(|_| PeerTransportError::Invalid)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn submission_id(
    parent: &ExecutionContextReference,
    binding_digest: &str,
    message: &TaskMessage,
) -> Uuid {
    let identity = serde_json::to_vec(&serde_json::json!({
        "domain":"acteon.peer-send.v1",
        "binding_digest":binding_digest,
        "message_id":message.message_id,
    }))
    .expect("fixed peer submission identity must serialize");
    Uuid::new_v5(&parent.execution_id(), &identity)
}

fn cancellation_id(submission_id: Uuid) -> Uuid {
    Uuid::new_v5(&submission_id, b"acteon.peer-cancel.v1")
}

fn continuation_id(submission_id: Uuid, challenge_id: &str) -> Uuid {
    let identity = serde_json::to_vec(&serde_json::json!({
        "domain":"acteon.peer-continuation.v1",
        "challenge_id":challenge_id,
    }))
    .expect("fixed peer continuation identity must serialize");
    Uuid::new_v5(&submission_id, &identity)
}

impl DurablePeerTransport {
    pub fn new_trusted(
        dependencies: PeerTransportDependencies,
        adapter: Arc<dyn PeerTransportAdapter>,
        timeout: Duration,
    ) -> Result<Self, PeerTransportError> {
        if timeout.is_zero()
            || !valid_text(adapter.revision())
            || !valid_digest(adapter.binding_digest())
        {
            return Err(PeerTransportError::Invalid);
        }
        Ok(Self {
            dependencies,
            adapter,
            timeout,
        })
    }

    pub async fn submit(
        &self,
        registry: &ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        message: &TaskMessage,
    ) -> Result<PeerSendReceipt, PeerTransportError> {
        let (binding, record) = self
            .prepare(registry, agent_id, skill, parent, permits, message)
            .await?;
        let key = Self::key(&record);
        let created = self
            .dependencies
            .state
            .check_and_set(&key, &self.encode(&record)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?;
        if !created {
            return self.observe_record(&key, &record).await;
        }
        self.claim_and_send(&key, record, binding, false).await
    }

    /// Observe an existing journal entry after repeating all current binding and
    /// source-authority checks. This never creates an intent or calls an adapter.
    pub async fn observe(
        &self,
        registry: &ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        message: &TaskMessage,
    ) -> Result<PeerSendReceipt, PeerTransportError> {
        let (_, expected) = self
            .prepare(registry, agent_id, skill, parent, permits, message)
            .await?;
        self.observe_record(&Self::key(&expected), &expected).await
    }

    /// Explicitly recover an ambiguous submission. This may issue another
    /// network call only for an adapter whose exact binding guarantees replay of
    /// the same message identity returns the original remote acceptance.
    pub async fn replay_idempotent(
        &self,
        registry: &ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        message: &TaskMessage,
    ) -> Result<PeerSendReceipt, PeerTransportError> {
        if self.adapter.submission_capability() != PeerSubmissionCapability::VerifiedIdempotent {
            return Err(PeerTransportError::Refused);
        }
        let (binding, expected) = self
            .prepare(registry, agent_id, skill, parent, permits, message)
            .await?;
        let key = Self::key(&expected);
        let (current, _) = self
            .load_versioned(&key)
            .await?
            .ok_or(PeerTransportError::Conflict)?;
        self.same_intent(&current, &expected)?;
        match current.state {
            SendState::Accepted { .. } | SendState::Rejected { .. } => Ok(Self::receipt(&current)),
            SendState::Registered | SendState::Delivering { .. } | SendState::Uncertain => {
                self.claim_and_send(&key, current, binding, true).await
            }
        }
    }

    /// Refresh a previously accepted remote task. The submission id is only a
    /// journal lookup: current source authority and the exact installed binding
    /// are rechecked before any network request. Failed observation never
    /// downgrades or erases the last durable remote task snapshot.
    pub async fn refresh_task(
        &self,
        registry: &ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        submission_id: Uuid,
    ) -> Result<PeerSendReceipt, PeerTransportError> {
        let binding = registry
            .bindings
            .get(&(agent_id.into(), skill.into()))
            .ok_or(PeerTransportError::Refused)?;
        if binding.digest() != self.adapter.binding_digest()
            || registry
                .inspect_binding(binding, self.dependencies.clock.as_ref())
                .await
                .map_err(|error| Self::discovery_error(&error))?
                .is_none()
        {
            return Err(PeerTransportError::Refused);
        }
        binding
            .check_source(
                &self.dependencies.coordinator,
                parent,
                permits,
                self.dependencies.clock.as_ref(),
            )
            .await
            .map_err(|_| PeerTransportError::Refused)?;
        let reference = parent
            .reference()
            .map_err(|_| PeerTransportError::Invalid)?;
        let key = StateKey::new(
            reference.namespace(),
            reference.tenant(),
            KeyKind::Custom(PEER_SEND_KIND.into()),
            submission_id.to_string(),
        );
        let (record, version) = self
            .load_versioned(&key)
            .await?
            .ok_or(PeerTransportError::Conflict)?;
        self.authorize_existing(&record, binding, &reference, submission_id)?;
        let SendState::Accepted {
            task: current,
            source_context,
            progress_cursor,
        } = &record.state
        else {
            return Ok(Self::receipt(&record));
        };
        if current.status.state.is_terminal() {
            return Ok(Self::receipt(&record));
        }
        let observed = tokio::time::timeout(
            self.timeout,
            self.adapter.observe_task(PeerTaskRequest {
                endpoint: &record.endpoint,
                transport: &record.transport,
                source_context,
                task_id: &current.id,
                progress_cursor: progress_cursor.as_deref(),
            }),
        )
        .await
        .map_err(|_| PeerTransportError::Unavailable)??;
        let Some(refreshed) = Self::apply_task_observation(&record, observed)? else {
            return Ok(Self::receipt(&record));
        };
        let encoded = self
            .encode(&refreshed)
            .map_err(|_| PeerTransportError::Unavailable)?;
        match self
            .dependencies
            .state
            .compare_and_swap(&key, version, &encoded, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => Ok(Self::receipt(&refreshed)),
            CasResult::Conflict { .. } => {
                let (current, _) = self
                    .load_versioned(&key)
                    .await?
                    .ok_or(PeerTransportError::Conflict)?;
                self.authorize_existing(&current, binding, &reference, submission_id)?;
                Ok(Self::receipt(&current))
            }
        }
    }

    /// Respond once to the exact active `InputRequired` challenge recorded for
    /// an accepted peer task. Callers supply message content only: task,
    /// context, and challenge authority are rejected and then injected from the
    /// durable remote snapshot. Intent and a delivery claim precede networking.
    pub async fn continue_task(
        &self,
        registry: &ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        input: PeerContinuationInput<'_>,
    ) -> Result<PeerContinuationReceipt, PeerTransportError> {
        let response = input.response;
        if response.task_id.is_some()
            || response.context_id.is_some()
            || response.role != TaskRole::User
            || response
                .metadata
                .contains_key(TASK_CHALLENGE_ID_METADATA_KEY)
        {
            return Err(PeerTransportError::Invalid);
        }
        response
            .validate()
            .map_err(|_| PeerTransportError::Invalid)?;
        let (binding, reference, send_key, send) = self
            .authorized_existing(
                registry,
                agent_id,
                skill,
                parent,
                permits,
                input.submission_id,
            )
            .await?;
        self.continue_authorized(AuthorizedContinuation {
            binding,
            reference,
            send_key,
            send,
            input,
        })
        .await
    }

    // Keep intent construction, durable claim selection, and accepted-snapshot
    // projection together so their fail-closed ordering remains reviewable.
    #[allow(clippy::too_many_lines)]
    async fn continue_authorized(
        &self,
        authorized: AuthorizedContinuation<'_>,
    ) -> Result<PeerContinuationReceipt, PeerTransportError> {
        let AuthorizedContinuation {
            binding,
            reference,
            send_key,
            send,
            input,
        } = authorized;
        let submission_id = input.submission_id;
        let response = input.response;
        let SendState::Accepted {
            task: current_task,
            source_context,
            progress_cursor,
        } = &send.state
        else {
            return Err(PeerTransportError::Conflict);
        };
        let (challenge_id, normalized) = Self::normalize_continuation(current_task, response)?;
        let existing_id = continuation_id(submission_id, &challenge_id);
        if current_task.status.state != TaskState::InputRequired {
            let key = Self::continuation_key(&reference, existing_id);
            let (record, _) = self
                .load_continuation_versioned(&key)
                .await?
                .ok_or(PeerTransportError::Conflict)?;
            if !self.valid_continuation_record(&record)
                || record.submission_id != submission_id
                || record.continuation_id != existing_id
                || record.binding_digest != binding.digest()
                || record.endpoint != binding.endpoint()
                || record.transport != binding.transport()
                || record.source_context != *source_context
                || record.task_id != current_task.id
                || record.challenge_id != challenge_id
                || record.message_digest != message_digest(&normalized)?
                || serde_json::to_value(&record.message)
                    .map_err(|_| PeerTransportError::Conflict)?
                    != serde_json::to_value(&normalized)
                        .map_err(|_| PeerTransportError::Conflict)?
            {
                return Err(PeerTransportError::Conflict);
            }
            return Ok(Self::continuation_receipt(&record));
        }
        let requested_at_ms = self.dependencies.clock.now().timestamp_millis();
        if requested_at_ms < 0 {
            return Err(PeerTransportError::Invalid);
        }
        let candidate = ContinuationRecord {
            schema: 1,
            submission_id,
            continuation_id: existing_id,
            binding_digest: binding.digest().into(),
            adapter_revision: self.adapter.revision().into(),
            endpoint: binding.endpoint().into(),
            transport: binding.transport().into(),
            source_context: source_context.clone(),
            task_id: current_task.id.clone(),
            challenge_id,
            prior_task: current_task.clone(),
            prior_progress_cursor: progress_cursor.clone(),
            message_digest: message_digest(&normalized)?,
            message: normalized,
            requested_at_ms,
            state: ContinuationState::Registered,
        };
        let key = Self::continuation_key(&reference, candidate.continuation_id);
        let created = self
            .dependencies
            .state
            .check_and_set(&key, &self.encode_continuation(&candidate)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?;
        let receipt = if created {
            self.claim_and_continue(&key, &candidate).await?
        } else {
            let (record, _) = self
                .load_continuation_versioned(&key)
                .await?
                .ok_or(PeerTransportError::Unavailable)?;
            self.same_continuation_intent(&record, &candidate)?;
            if matches!(record.state, ContinuationState::Registered) {
                self.claim_and_continue(&key, &record).await?
            } else {
                Self::continuation_receipt(&record)
            }
        };
        if let PeerContinuationStatus::Accepted {
            task,
            progress_cursor,
        } = &receipt.status
        {
            self.persist_continued_task(ContinuedTaskProjection {
                key: &send_key,
                binding,
                parent: &reference,
                continuation: &candidate,
                continued_task: task,
                progress_cursor,
            })
            .await?;
        }
        Ok(receipt)
    }

    fn normalize_continuation(
        task: &Task,
        response: &TaskMessage,
    ) -> Result<(String, TaskMessage), PeerTransportError> {
        let challenge_id = if task.status.state == TaskState::InputRequired {
            task.pending_approval_id
                .clone()
                .filter(|id| valid_text(id))
                .ok_or(PeerTransportError::Conflict)?
        } else {
            task.history
                .iter()
                .find(|message| message.message_id == response.message_id)
                .and_then(|message| {
                    message
                        .metadata
                        .get(TASK_CHALLENGE_ID_METADATA_KEY)
                        .and_then(serde_json::Value::as_str)
                })
                .filter(|id| valid_text(id))
                .map(str::to_owned)
                .ok_or(PeerTransportError::Conflict)?
        };
        let mut normalized = response.clone();
        normalized.task_id = Some(task.id.clone());
        normalized.context_id.clone_from(&task.context_id);
        normalized.metadata.insert(
            TASK_CHALLENGE_ID_METADATA_KEY.into(),
            serde_json::Value::String(challenge_id.clone()),
        );
        normalized
            .validate_in_task(&task.id)
            .map_err(|_| PeerTransportError::Invalid)?;
        if task.status.state != TaskState::InputRequired {
            let applied = task
                .history
                .iter()
                .find(|message| message.message_id == normalized.message_id)
                .ok_or(PeerTransportError::Conflict)?;
            if serde_json::to_value(applied).map_err(|_| PeerTransportError::Conflict)?
                != serde_json::to_value(&normalized).map_err(|_| PeerTransportError::Conflict)?
            {
                return Err(PeerTransportError::Conflict);
            }
        }
        Ok((challenge_id, normalized))
    }

    fn apply_task_observation(
        record: &SendRecord,
        observed: PeerTaskObservation,
    ) -> Result<Option<SendRecord>, PeerTransportError> {
        let SendState::Accepted {
            task: current,
            source_context,
            progress_cursor,
        } = &record.state
        else {
            return Err(PeerTransportError::Conflict);
        };
        let (observed, observed_cursor) = match observed {
            PeerTaskObservation::Unchanged {
                progress_cursor: observed_cursor,
            } => {
                if progress_cursor.as_deref() != Some(observed_cursor.as_str())
                    || !valid_progress_cursor(&observed_cursor)
                {
                    return Err(PeerTransportError::Unavailable);
                }
                return Ok(None);
            }
            PeerTaskObservation::Updated {
                task,
                progress_cursor,
            } => (task, progress_cursor),
        };
        if observed_cursor
            .as_deref()
            .is_some_and(|cursor| !valid_progress_cursor(cursor))
            || (progress_cursor.is_some() && observed_cursor.is_none())
            || !valid_task_progress(current, &observed)
        {
            return Err(PeerTransportError::Unavailable);
        }
        let task_unchanged = serde_json::to_value(current)
            .map_err(|_| PeerTransportError::Unavailable)?
            == serde_json::to_value(&observed).map_err(|_| PeerTransportError::Unavailable)?;
        if progress_cursor == &observed_cursor {
            return if task_unchanged {
                Ok(None)
            } else {
                Err(PeerTransportError::Unavailable)
            };
        }
        let mut refreshed = record.clone();
        refreshed.schema = PEER_SEND_SCHEMA;
        refreshed.state = SendState::Accepted {
            task: observed,
            source_context: source_context.clone(),
            progress_cursor: observed_cursor,
        };
        Ok(Some(refreshed))
    }

    /// Deliver at most one cancellation request for an accepted remote task.
    /// Intent and a delivery claim are durable before the adapter call. A lost
    /// response or crash after that claim remains uncertain and is never
    /// automatically resent.
    pub async fn cancel_task(
        &self,
        registry: &ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        submission_id: Uuid,
    ) -> Result<PeerCancelReceipt, PeerTransportError> {
        let (binding, reference, send_key, send) = self
            .authorized_existing(registry, agent_id, skill, parent, permits, submission_id)
            .await?;
        let SendState::Accepted {
            task: accepted_task,
            source_context,
            ..
        } = &send.state
        else {
            return Err(PeerTransportError::Conflict);
        };
        let requested_at_ms = self.dependencies.clock.now().timestamp_millis();
        if requested_at_ms < 0 {
            return Err(PeerTransportError::Invalid);
        }
        let final_task = accepted_task
            .status
            .state
            .is_terminal()
            .then(|| accepted_task.clone());
        let candidate = CancelRecord {
            schema: 1,
            submission_id,
            cancellation_id: cancellation_id(submission_id),
            binding_digest: binding.digest().into(),
            adapter_revision: self.adapter.revision().into(),
            endpoint: binding.endpoint().into(),
            transport: binding.transport().into(),
            source_context: source_context.clone(),
            task_id: accepted_task.id.clone(),
            requested_at_ms,
            state: final_task.map_or(CancelState::Registered, |task| CancelState::Reconciled {
                task,
            }),
        };
        let key = Self::cancel_key(&reference, candidate.cancellation_id);
        let created = self
            .dependencies
            .state
            .check_and_set(&key, &self.encode_cancel(&candidate)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?;
        let mut receipt = if created && matches!(candidate.state, CancelState::Registered) {
            self.claim_and_cancel(&key, &candidate, accepted_task)
                .await?
        } else if created {
            Self::cancel_receipt(&candidate)
        } else {
            let (current, _) = self
                .load_cancel_versioned(&key)
                .await?
                .ok_or(PeerTransportError::Unavailable)?;
            self.same_cancel_intent(&current, &candidate)?;
            if matches!(current.state, CancelState::Registered) {
                self.claim_and_cancel(&key, &current, accepted_task).await?
            } else {
                Self::cancel_receipt(&current)
            }
        };
        if !created
            && matches!(
                receipt.status,
                PeerCancelStatus::Uncertain | PeerCancelStatus::Restricted { .. }
            )
            && let Ok(refreshed) = self
                .refresh_task(registry, agent_id, skill, parent, permits, submission_id)
                .await
            && let PeerSendStatus::Accepted { task: observed, .. } = refreshed.status
            && valid_task_progress(accepted_task, &observed)
        {
            if observed.status.state.is_terminal() {
                receipt = self
                    .reconcile_cancel(&key, &candidate, observed.as_ref())
                    .await?;
            } else if matches!(receipt.status, PeerCancelStatus::Restricted { .. }) {
                receipt = self
                    .refresh_restricted_cancel(&key, &candidate, observed.as_ref())
                    .await?;
            }
        }
        if let PeerCancelStatus::Reconciled { task } = &receipt.status {
            self.persist_final_task(&send_key, binding, &reference, submission_id, task)
                .await?;
        }
        Ok(receipt)
    }

    async fn authorized_existing<'a>(
        &self,
        registry: &'a ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        submission_id: Uuid,
    ) -> Result<
        (
            &'a ApprovedPeerBinding,
            ExecutionContextReference,
            StateKey,
            SendRecord,
        ),
        PeerTransportError,
    > {
        let binding = registry
            .bindings
            .get(&(agent_id.into(), skill.into()))
            .ok_or(PeerTransportError::Refused)?;
        if binding.digest() != self.adapter.binding_digest()
            || registry
                .inspect_binding(binding, self.dependencies.clock.as_ref())
                .await
                .map_err(|error| Self::discovery_error(&error))?
                .is_none()
        {
            return Err(PeerTransportError::Refused);
        }
        binding
            .check_source(
                &self.dependencies.coordinator,
                parent,
                permits,
                self.dependencies.clock.as_ref(),
            )
            .await
            .map_err(|_| PeerTransportError::Refused)?;
        let reference = parent
            .reference()
            .map_err(|_| PeerTransportError::Invalid)?;
        let key = StateKey::new(
            reference.namespace(),
            reference.tenant(),
            KeyKind::Custom(PEER_SEND_KIND.into()),
            submission_id.to_string(),
        );
        let (record, _) = self
            .load_versioned(&key)
            .await?
            .ok_or(PeerTransportError::Conflict)?;
        self.authorize_existing(&record, binding, &reference, submission_id)?;
        Ok((binding, reference, key, record))
    }

    async fn claim_and_continue(
        &self,
        key: &StateKey,
        expected: &ContinuationRecord,
    ) -> Result<PeerContinuationReceipt, PeerTransportError> {
        let (current, version) = self
            .load_continuation_versioned(key)
            .await?
            .ok_or(PeerTransportError::Unavailable)?;
        self.same_continuation_intent(&current, expected)?;
        if !matches!(current.state, ContinuationState::Registered) {
            return Ok(Self::continuation_receipt(&current));
        }
        let mut claimed = current.clone();
        claimed.state = ContinuationState::Delivering {
            claim_id: Uuid::new_v4(),
        };
        match self
            .dependencies
            .state
            .compare_and_swap(key, version, &self.encode_continuation(&claimed)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => {}
            CasResult::Conflict { .. } => {
                let (record, _) = self
                    .load_continuation_versioned(key)
                    .await?
                    .ok_or(PeerTransportError::Unavailable)?;
                self.same_continuation_intent(&record, expected)?;
                return Ok(Self::continuation_receipt(&record));
            }
        }
        let delivery = tokio::time::timeout(
            self.timeout,
            self.adapter.continue_task(PeerContinuationRequest {
                endpoint: &claimed.endpoint,
                transport: &claimed.transport,
                source_context: &claimed.source_context,
                task_id: &claimed.task_id,
                challenge_id: &claimed.challenge_id,
                progress_cursor: claimed.prior_progress_cursor.as_deref(),
                message: &claimed.message,
            }),
        )
        .await;
        let state = match delivery {
            Ok(Ok(PeerContinuationDisposition::Accepted {
                task,
                progress_cursor,
            })) if Self::valid_continuation_result(
                &claimed,
                &claimed.prior_task,
                &task,
                &progress_cursor,
            ) =>
            {
                ContinuationState::Accepted {
                    task,
                    progress_cursor,
                }
            }
            Ok(Ok(PeerContinuationDisposition::Rejected { code })) if valid_text(&code) => {
                ContinuationState::Rejected { code }
            }
            _ => ContinuationState::Uncertain,
        };
        let mut settled = claimed;
        settled.state = state;
        let encoded = self
            .encode_continuation(&settled)
            .map_err(|_| PeerTransportError::Unavailable)?;
        match self
            .dependencies
            .state
            .compare_and_swap(key, version + 1, &encoded, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => Ok(Self::continuation_receipt(&settled)),
            CasResult::Conflict { .. } => {
                let (record, _) = self
                    .load_continuation_versioned(key)
                    .await?
                    .ok_or(PeerTransportError::Unavailable)?;
                self.same_continuation_intent(&record, expected)?;
                Ok(Self::continuation_receipt(&record))
            }
        }
    }

    fn valid_continuation_result(
        record: &ContinuationRecord,
        prior: &Task,
        observed: &Task,
        cursor: &str,
    ) -> bool {
        valid_progress_cursor(cursor)
            && record.prior_progress_cursor.as_deref() != Some(cursor)
            && valid_task_progress(prior, observed)
            && observed.updated_at > prior.updated_at
            && observed.history.iter().any(|message| {
                message.message_id == record.message.message_id
                    && serde_json::to_value(message).ok()
                        == serde_json::to_value(&record.message).ok()
            })
            && (observed.status.state != TaskState::InputRequired
                || observed.pending_approval_id.as_deref() != Some(record.challenge_id.as_str()))
    }

    async fn persist_continued_task(
        &self,
        projection: ContinuedTaskProjection<'_>,
    ) -> Result<(), PeerTransportError> {
        let ContinuedTaskProjection {
            key,
            binding,
            parent,
            continuation,
            continued_task,
            progress_cursor,
        } = projection;
        for _ in 0..16 {
            let (current, version) = self
                .load_versioned(key)
                .await?
                .ok_or(PeerTransportError::Conflict)?;
            self.authorize_existing(&current, binding, parent, continuation.submission_id)?;
            let SendState::Accepted {
                task,
                source_context,
                progress_cursor: current_cursor,
            } = &current.state
            else {
                return Err(PeerTransportError::Conflict);
            };
            if serde_json::to_value(task).map_err(|_| PeerTransportError::Unavailable)?
                == serde_json::to_value(continued_task)
                    .map_err(|_| PeerTransportError::Unavailable)?
                && current_cursor.as_deref() == Some(progress_cursor)
            {
                return Ok(());
            }
            if valid_task_progress(continued_task, task) {
                return if task.history.iter().any(|message| {
                    message.message_id == continuation.message.message_id
                        && serde_json::to_value(message).ok()
                            == serde_json::to_value(&continuation.message).ok()
                }) {
                    Ok(())
                } else {
                    Err(PeerTransportError::Conflict)
                };
            }
            if serde_json::to_value(task).map_err(|_| PeerTransportError::Unavailable)?
                != serde_json::to_value(&continuation.prior_task)
                    .map_err(|_| PeerTransportError::Unavailable)?
                || source_context != &continuation.source_context
                || current_cursor != &continuation.prior_progress_cursor
                || !Self::valid_continuation_result(
                    continuation,
                    &continuation.prior_task,
                    continued_task,
                    progress_cursor,
                )
            {
                return Err(PeerTransportError::Conflict);
            }
            let mut next = current;
            next.schema = PEER_SEND_SCHEMA;
            next.state = SendState::Accepted {
                task: Box::new(continued_task.clone()),
                source_context: continuation.source_context.clone(),
                progress_cursor: Some(progress_cursor.into()),
            };
            if matches!(
                self.dependencies
                    .state
                    .compare_and_swap(key, version, &self.encode(&next)?, None)
                    .await
                    .map_err(|_| PeerTransportError::Unavailable)?,
                CasResult::Ok
            ) {
                return Ok(());
            }
        }
        Err(PeerTransportError::Unavailable)
    }

    async fn claim_and_cancel(
        &self,
        key: &StateKey,
        expected: &CancelRecord,
        current_task: &Task,
    ) -> Result<PeerCancelReceipt, PeerTransportError> {
        let (current, version) = self
            .load_cancel_versioned(key)
            .await?
            .ok_or(PeerTransportError::Unavailable)?;
        self.same_cancel_intent(&current, expected)?;
        if !matches!(current.state, CancelState::Registered) {
            return Ok(Self::cancel_receipt(&current));
        }
        let mut claimed = current.clone();
        claimed.state = CancelState::Delivering {
            claim_id: Uuid::new_v4(),
        };
        match self
            .dependencies
            .state
            .compare_and_swap(key, version, &self.encode_cancel(&claimed)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => {}
            CasResult::Conflict { .. } => {
                let (record, _) = self
                    .load_cancel_versioned(key)
                    .await?
                    .ok_or(PeerTransportError::Unavailable)?;
                self.same_cancel_intent(&record, expected)?;
                return Ok(Self::cancel_receipt(&record));
            }
        }
        let delivery = tokio::time::timeout(
            self.timeout,
            self.adapter.cancel_task(PeerTaskRequest {
                endpoint: &claimed.endpoint,
                transport: &claimed.transport,
                source_context: &claimed.source_context,
                task_id: &claimed.task_id,
                progress_cursor: None,
            }),
        )
        .await;
        let state = match delivery {
            Ok(Ok(PeerCancelDisposition::Unsupported)) => CancelState::Unsupported,
            Ok(Ok(PeerCancelDisposition::Rejected { code })) if valid_text(&code) => {
                CancelState::Rejected { code }
            }
            Ok(Ok(PeerCancelDisposition::Restricted { task }))
                if !task.status.state.is_terminal() && valid_task_progress(current_task, &task) =>
            {
                CancelState::Restricted { task }
            }
            Ok(Ok(PeerCancelDisposition::Final { task }))
                if task.status.state.is_terminal() && valid_task_progress(current_task, &task) =>
            {
                CancelState::Reconciled { task }
            }
            _ => CancelState::Uncertain,
        };
        let mut settled = claimed;
        settled.state = state;
        let encoded = self
            .encode_cancel(&settled)
            .map_err(|_| PeerTransportError::Unavailable)?;
        match self
            .dependencies
            .state
            .compare_and_swap(key, version + 1, &encoded, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => Ok(Self::cancel_receipt(&settled)),
            CasResult::Conflict { .. } => {
                let (record, _) = self
                    .load_cancel_versioned(key)
                    .await?
                    .ok_or(PeerTransportError::Unavailable)?;
                self.same_cancel_intent(&record, expected)?;
                Ok(Self::cancel_receipt(&record))
            }
        }
    }

    async fn reconcile_cancel(
        &self,
        key: &StateKey,
        expected: &CancelRecord,
        final_task: &Task,
    ) -> Result<PeerCancelReceipt, PeerTransportError> {
        for _ in 0..16 {
            let (current, version) = self
                .load_cancel_versioned(key)
                .await?
                .ok_or(PeerTransportError::Unavailable)?;
            self.same_cancel_intent(&current, expected)?;
            match current.state {
                CancelState::Uncertain
                | CancelState::Delivering { .. }
                | CancelState::Restricted { .. } => {}
                _ => return Ok(Self::cancel_receipt(&current)),
            }
            if !final_task.status.state.is_terminal()
                || final_task.id != current.task_id
                || final_task.namespace != current.source_context.namespace()
                || final_task.tenant != current.source_context.tenant()
            {
                return Err(PeerTransportError::Conflict);
            }
            let mut next = current;
            next.state = CancelState::Reconciled {
                task: Box::new(final_task.clone()),
            };
            let encoded = self
                .encode_cancel(&next)
                .map_err(|_| PeerTransportError::Unavailable)?;
            if matches!(
                self.dependencies
                    .state
                    .compare_and_swap(key, version, &encoded, None)
                    .await
                    .map_err(|_| PeerTransportError::Unavailable)?,
                CasResult::Ok
            ) {
                return Ok(Self::cancel_receipt(&next));
            }
        }
        Err(PeerTransportError::Unavailable)
    }

    async fn refresh_restricted_cancel(
        &self,
        key: &StateKey,
        expected: &CancelRecord,
        observed: &Task,
    ) -> Result<PeerCancelReceipt, PeerTransportError> {
        for _ in 0..16 {
            let (current, version) = self
                .load_cancel_versioned(key)
                .await?
                .ok_or(PeerTransportError::Unavailable)?;
            self.same_cancel_intent(&current, expected)?;
            let CancelState::Restricted { task } = &current.state else {
                return Ok(Self::cancel_receipt(&current));
            };
            if observed.status.state.is_terminal() || !valid_task_progress(task, observed) {
                return Err(PeerTransportError::Conflict);
            }
            let mut next = current;
            next.state = CancelState::Restricted {
                task: Box::new(observed.clone()),
            };
            let encoded = self
                .encode_cancel(&next)
                .map_err(|_| PeerTransportError::Unavailable)?;
            if matches!(
                self.dependencies
                    .state
                    .compare_and_swap(key, version, &encoded, None)
                    .await
                    .map_err(|_| PeerTransportError::Unavailable)?,
                CasResult::Ok
            ) {
                return Ok(Self::cancel_receipt(&next));
            }
        }
        Err(PeerTransportError::Unavailable)
    }

    async fn prepare<'a>(
        &self,
        registry: &'a ApprovedPeerRegistry,
        agent_id: &str,
        skill: &str,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        message: &TaskMessage,
    ) -> Result<(&'a ApprovedPeerBinding, SendRecord), PeerTransportError> {
        message
            .validate()
            .map_err(|_| PeerTransportError::Invalid)?;
        if message.task_id.is_some()
            || permits.is_empty()
            || !Arc::ptr_eq(&self.dependencies.state, &registry.state)
        {
            return Err(PeerTransportError::Invalid);
        }
        let binding = registry
            .bindings
            .get(&(agent_id.into(), skill.into()))
            .ok_or(PeerTransportError::Refused)?;
        if binding.digest() != self.adapter.binding_digest()
            || registry
                .inspect_binding(binding, self.dependencies.clock.as_ref())
                .await
                .map_err(|error| Self::discovery_error(&error))?
                .is_none()
        {
            return Err(PeerTransportError::Refused);
        }
        binding
            .check_source(
                &self.dependencies.coordinator,
                parent,
                permits,
                self.dependencies.clock.as_ref(),
            )
            .await
            .map_err(|_| PeerTransportError::Refused)?;
        let reference = parent
            .reference()
            .map_err(|_| PeerTransportError::Invalid)?;
        let digest = message_digest(message)?;
        let mut selected = permits.to_vec();
        selected.sort_by(|a, b| a.id.cmp(&b.id));
        let created_at_ms = self.dependencies.clock.now().timestamp_millis();
        if created_at_ms < 0 {
            return Err(PeerTransportError::Invalid);
        }
        Ok((
            binding,
            SendRecord {
                schema: PEER_SEND_SCHEMA,
                submission_id: submission_id(&reference, binding.digest(), message),
                binding_digest: binding.digest().into(),
                adapter_revision: self.adapter.revision().into(),
                capability: self.adapter.submission_capability(),
                endpoint: binding.endpoint().into(),
                transport: binding.transport().into(),
                parent: reference,
                permits: selected,
                message: message.clone(),
                message_digest: digest,
                created_at_ms,
                state: SendState::Registered,
            },
        ))
    }

    fn discovery_error(error: &PeerDiscoveryError) -> PeerTransportError {
        match error {
            PeerDiscoveryError::Binding | PeerDiscoveryError::Capacity => {
                PeerTransportError::Refused
            }
            PeerDiscoveryError::Unavailable | PeerDiscoveryError::Authority(_) => {
                PeerTransportError::Unavailable
            }
        }
    }

    fn key(record: &SendRecord) -> StateKey {
        StateKey::new(
            record.parent.namespace(),
            record.parent.tenant(),
            KeyKind::Custom(PEER_SEND_KIND.into()),
            record.submission_id.to_string(),
        )
    }

    fn cancel_key(parent: &ExecutionContextReference, cancellation_id: Uuid) -> StateKey {
        StateKey::new(
            parent.namespace(),
            parent.tenant(),
            KeyKind::Custom(PEER_CANCEL_KIND.into()),
            cancellation_id.to_string(),
        )
    }

    fn continuation_key(parent: &ExecutionContextReference, continuation_id: Uuid) -> StateKey {
        StateKey::new(
            parent.namespace(),
            parent.tenant(),
            KeyKind::Custom(PEER_CONTINUATION_KIND.into()),
            continuation_id.to_string(),
        )
    }

    async fn claim_and_send(
        &self,
        key: &StateKey,
        expected: SendRecord,
        binding: &ApprovedPeerBinding,
        replay: bool,
    ) -> Result<PeerSendReceipt, PeerTransportError> {
        let (current, version) = self
            .load_versioned(key)
            .await?
            .ok_or(PeerTransportError::Unavailable)?;
        self.same_intent(&current, &expected)?;
        if !replay && !matches!(current.state, SendState::Registered) {
            return Ok(Self::receipt(&current));
        }
        if replay
            && self.adapter.submission_capability() != PeerSubmissionCapability::VerifiedIdempotent
        {
            return Err(PeerTransportError::Refused);
        }
        let mut claimed = current.clone();
        claimed.schema = PEER_SEND_SCHEMA;
        claimed.state = SendState::Delivering {
            claim_id: Uuid::new_v4(),
        };
        match self
            .dependencies
            .state
            .compare_and_swap(key, version, &self.encode(&claimed)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => {}
            CasResult::Conflict { .. } => return self.observe_record(key, &expected).await,
        }
        let disposition = tokio::time::timeout(
            self.timeout,
            self.adapter.send(PeerSendRequest {
                endpoint: binding.endpoint(),
                transport: binding.transport(),
                parent: &claimed.parent,
                permits: &claimed.permits,
                message: &claimed.message,
            }),
        )
        .await;
        let state = match disposition {
            Ok(Ok(PeerSendDisposition::Accepted {
                task,
                source_context,
                progress_cursor,
            })) if task.validate().is_ok()
                && task.namespace == claimed.parent.namespace()
                && task.tenant == claimed.parent.tenant()
                && source_context == claimed.parent
                && progress_cursor.as_deref().is_none_or(valid_progress_cursor) =>
            {
                SendState::Accepted {
                    task,
                    source_context,
                    progress_cursor,
                }
            }
            Ok(Ok(PeerSendDisposition::Rejected { code })) if valid_text(&code) => {
                SendState::Rejected { code }
            }
            _ => SendState::Uncertain,
        };
        let mut settled = claimed.clone();
        settled.state = state;
        match self
            .dependencies
            .state
            .compare_and_swap(key, version + 1, &self.encode(&settled)?, None)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
        {
            CasResult::Ok => Ok(Self::receipt(&settled)),
            CasResult::Conflict { .. } => self.observe_record(key, &expected).await,
        }
    }

    async fn observe_record(
        &self,
        key: &StateKey,
        expected: &SendRecord,
    ) -> Result<PeerSendReceipt, PeerTransportError> {
        let (record, _) = self
            .load_versioned(key)
            .await?
            .ok_or(PeerTransportError::Unavailable)?;
        self.same_intent(&record, expected)?;
        Ok(Self::receipt(&record))
    }

    fn same_intent(
        &self,
        actual: &SendRecord,
        expected: &SendRecord,
    ) -> Result<(), PeerTransportError> {
        if !self.valid_record(actual) || !self.valid_record(expected) {
            return Err(PeerTransportError::Conflict);
        }
        let mut actual = actual.clone();
        let mut expected = expected.clone();
        actual.schema = PEER_SEND_SCHEMA;
        expected.schema = PEER_SEND_SCHEMA;
        actual.state = SendState::Registered;
        expected.state = SendState::Registered;
        actual.created_at_ms = 0;
        expected.created_at_ms = 0;
        let actual_value =
            serde_json::to_value(&actual).map_err(|_| PeerTransportError::Conflict)?;
        let expected_value =
            serde_json::to_value(&expected).map_err(|_| PeerTransportError::Conflict)?;
        if actual_value != expected_value {
            return Err(PeerTransportError::Conflict);
        }
        Ok(())
    }

    fn authorize_existing(
        &self,
        record: &SendRecord,
        binding: &ApprovedPeerBinding,
        parent: &ExecutionContextReference,
        submission_id: Uuid,
    ) -> Result<(), PeerTransportError> {
        if !self.valid_record(record)
            || record.submission_id != submission_id
            || &record.parent != parent
            || record.binding_digest != binding.digest()
            || record.endpoint != binding.endpoint()
            || record.transport != binding.transport()
        {
            return Err(PeerTransportError::Conflict);
        }
        Ok(())
    }

    fn same_cancel_intent(
        &self,
        actual: &CancelRecord,
        expected: &CancelRecord,
    ) -> Result<(), PeerTransportError> {
        if !self.valid_cancel_record(actual)
            || !self.valid_cancel_record(expected)
            || actual.schema != expected.schema
            || actual.submission_id != expected.submission_id
            || actual.cancellation_id != expected.cancellation_id
            || actual.binding_digest != expected.binding_digest
            || actual.adapter_revision != expected.adapter_revision
            || actual.endpoint != expected.endpoint
            || actual.transport != expected.transport
            || actual.source_context != expected.source_context
            || actual.task_id != expected.task_id
        {
            return Err(PeerTransportError::Conflict);
        }
        Ok(())
    }

    fn same_continuation_intent(
        &self,
        actual: &ContinuationRecord,
        expected: &ContinuationRecord,
    ) -> Result<(), PeerTransportError> {
        if !self.valid_continuation_record(actual)
            || !self.valid_continuation_record(expected)
            || actual.schema != expected.schema
            || actual.submission_id != expected.submission_id
            || actual.continuation_id != expected.continuation_id
            || actual.binding_digest != expected.binding_digest
            || actual.adapter_revision != expected.adapter_revision
            || actual.endpoint != expected.endpoint
            || actual.transport != expected.transport
            || actual.source_context != expected.source_context
            || actual.task_id != expected.task_id
            || actual.challenge_id != expected.challenge_id
            || serde_json::to_value(&actual.prior_task).map_err(|_| PeerTransportError::Conflict)?
                != serde_json::to_value(&expected.prior_task)
                    .map_err(|_| PeerTransportError::Conflict)?
            || actual.prior_progress_cursor != expected.prior_progress_cursor
            || actual.message_digest != expected.message_digest
            || serde_json::to_value(&actual.message).map_err(|_| PeerTransportError::Conflict)?
                != serde_json::to_value(&expected.message)
                    .map_err(|_| PeerTransportError::Conflict)?
        {
            return Err(PeerTransportError::Conflict);
        }
        Ok(())
    }

    fn valid_continuation_record(&self, record: &ContinuationRecord) -> bool {
        let state_valid = match &record.state {
            ContinuationState::Rejected { code } => valid_text(code),
            ContinuationState::Accepted {
                task,
                progress_cursor,
            } => {
                task.validate().is_ok()
                    && task.id == record.task_id
                    && task.namespace == record.source_context.namespace()
                    && task.tenant == record.source_context.tenant()
                    && valid_progress_cursor(progress_cursor)
                    && record.prior_progress_cursor.as_deref() != Some(progress_cursor)
                    && task.history.iter().any(|message| {
                        message.message_id == record.message.message_id
                            && serde_json::to_value(message).ok()
                                == serde_json::to_value(&record.message).ok()
                    })
            }
            ContinuationState::Registered
            | ContinuationState::Delivering { .. }
            | ContinuationState::Uncertain => true,
        };
        record.schema == 1
            && record.continuation_id == continuation_id(record.submission_id, &record.challenge_id)
            && record.binding_digest == self.adapter.binding_digest()
            && record.adapter_revision == self.adapter.revision()
            && valid_digest(&record.binding_digest)
            && valid_text(&record.adapter_revision)
            && valid_endpoint(&record.endpoint)
            && matches!(record.transport.as_str(), "rest" | "json-rpc")
            && valid_text(&record.task_id)
            && valid_text(&record.challenge_id)
            && record.prior_task.validate().is_ok()
            && record.prior_task.id == record.task_id
            && record.prior_task.namespace == record.source_context.namespace()
            && record.prior_task.tenant == record.source_context.tenant()
            && record.prior_task.status.state == TaskState::InputRequired
            && record.prior_task.pending_approval_id.as_deref()
                == Some(record.challenge_id.as_str())
            && record
                .prior_progress_cursor
                .as_deref()
                .is_none_or(valid_progress_cursor)
            && record.message.validate_in_task(&record.task_id).is_ok()
            && record.message.role == TaskRole::User
            && record.message.task_id.as_deref() == Some(record.task_id.as_str())
            && record
                .message
                .metadata
                .get(TASK_CHALLENGE_ID_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                == Some(record.challenge_id.as_str())
            && valid_digest(&record.message_digest)
            && message_digest(&record.message).is_ok_and(|digest| digest == record.message_digest)
            && record.requested_at_ms >= 0
            && match &record.state {
                ContinuationState::Accepted {
                    task,
                    progress_cursor,
                } => Self::valid_continuation_result(
                    record,
                    &record.prior_task,
                    task,
                    progress_cursor,
                ),
                _ => true,
            }
            && state_valid
    }

    fn continuation_receipt(record: &ContinuationRecord) -> PeerContinuationReceipt {
        let status = match &record.state {
            ContinuationState::Rejected { code } => {
                PeerContinuationStatus::Rejected { code: code.clone() }
            }
            ContinuationState::Accepted {
                task,
                progress_cursor,
            } => PeerContinuationStatus::Accepted {
                task: task.clone(),
                progress_cursor: progress_cursor.clone(),
            },
            ContinuationState::Registered
            | ContinuationState::Delivering { .. }
            | ContinuationState::Uncertain => PeerContinuationStatus::Uncertain,
        };
        PeerContinuationReceipt {
            submission_id: record.submission_id,
            continuation_id: record.continuation_id,
            status,
        }
    }

    fn valid_cancel_record(&self, record: &CancelRecord) -> bool {
        let state_valid = match &record.state {
            CancelState::Rejected { code } => valid_text(code),
            CancelState::Restricted { task } => {
                task.validate().is_ok()
                    && !task.status.state.is_terminal()
                    && task.id == record.task_id
                    && task.namespace == record.source_context.namespace()
                    && task.tenant == record.source_context.tenant()
            }
            CancelState::Reconciled { task } => {
                task.validate().is_ok()
                    && task.status.state.is_terminal()
                    && task.id == record.task_id
                    && task.namespace == record.source_context.namespace()
                    && task.tenant == record.source_context.tenant()
            }
            CancelState::Registered
            | CancelState::Delivering { .. }
            | CancelState::Unsupported
            | CancelState::Uncertain => true,
        };
        record.schema == 1
            && record.cancellation_id == cancellation_id(record.submission_id)
            && record.binding_digest == self.adapter.binding_digest()
            && record.adapter_revision == self.adapter.revision()
            && valid_digest(&record.binding_digest)
            && valid_text(&record.adapter_revision)
            && valid_endpoint(&record.endpoint)
            && matches!(record.transport.as_str(), "rest" | "json-rpc")
            && valid_text(&record.task_id)
            && record.requested_at_ms >= 0
            && state_valid
    }

    fn cancel_receipt(record: &CancelRecord) -> PeerCancelReceipt {
        let status = match &record.state {
            CancelState::Unsupported => PeerCancelStatus::Unsupported,
            CancelState::Rejected { code } => PeerCancelStatus::Rejected { code: code.clone() },
            CancelState::Restricted { task } => PeerCancelStatus::Restricted { task: task.clone() },
            CancelState::Reconciled { task } => PeerCancelStatus::Reconciled { task: task.clone() },
            CancelState::Registered | CancelState::Delivering { .. } | CancelState::Uncertain => {
                PeerCancelStatus::Uncertain
            }
        };
        PeerCancelReceipt {
            submission_id: record.submission_id,
            cancellation_id: record.cancellation_id,
            status,
        }
    }

    async fn persist_final_task(
        &self,
        key: &StateKey,
        binding: &ApprovedPeerBinding,
        parent: &ExecutionContextReference,
        submission_id: Uuid,
        final_task: &Task,
    ) -> Result<(), PeerTransportError> {
        for _ in 0..16 {
            let (current, version) = self
                .load_versioned(key)
                .await?
                .ok_or(PeerTransportError::Conflict)?;
            self.authorize_existing(&current, binding, parent, submission_id)?;
            let SendState::Accepted {
                task,
                source_context,
                progress_cursor: _,
            } = &current.state
            else {
                return Err(PeerTransportError::Conflict);
            };
            if serde_json::to_value(task).map_err(|_| PeerTransportError::Unavailable)?
                == serde_json::to_value(final_task).map_err(|_| PeerTransportError::Unavailable)?
            {
                return Ok(());
            }
            if !final_task.status.state.is_terminal() || !valid_task_progress(task, final_task) {
                return Err(PeerTransportError::Unavailable);
            }
            let mut next = current.clone();
            next.schema = PEER_SEND_SCHEMA;
            next.state = SendState::Accepted {
                task: Box::new(final_task.clone()),
                source_context: source_context.clone(),
                // The cancellation response carries a newer task snapshot but
                // no observation validator. The prior cursor names the
                // pre-cancel representation and must not be retained.
                progress_cursor: None,
            };
            let encoded = self
                .encode(&next)
                .map_err(|_| PeerTransportError::Unavailable)?;
            if matches!(
                self.dependencies
                    .state
                    .compare_and_swap(key, version, &encoded, None)
                    .await
                    .map_err(|_| PeerTransportError::Unavailable)?,
                CasResult::Ok
            ) {
                return Ok(());
            }
        }
        Err(PeerTransportError::Unavailable)
    }

    fn valid_record(&self, record: &SendRecord) -> bool {
        let state_valid = match &record.state {
            SendState::Registered | SendState::Delivering { .. } | SendState::Uncertain => true,
            SendState::Accepted {
                task,
                source_context,
                progress_cursor,
            } => {
                task.validate().is_ok()
                    && task.namespace == record.parent.namespace()
                    && task.tenant == record.parent.tenant()
                    && source_context == &record.parent
                    && progress_cursor.as_deref().is_none_or(valid_progress_cursor)
            }
            SendState::Rejected { code } => valid_text(code),
        };
        matches!(record.schema, 1 | PEER_SEND_SCHEMA)
            && (record.schema != 1
                || !matches!(
                    &record.state,
                    SendState::Accepted {
                        progress_cursor: Some(_),
                        ..
                    }
                ))
            && record.binding_digest == self.adapter.binding_digest()
            && record.adapter_revision == self.adapter.revision()
            && record.capability == self.adapter.submission_capability()
            && valid_digest(&record.binding_digest)
            && valid_text(&record.adapter_revision)
            && valid_endpoint(&record.endpoint)
            && matches!(record.transport.as_str(), "rest" | "json-rpc")
            && valid_digest(&record.message_digest)
            && record.message.validate().is_ok()
            && record.message.task_id.is_none()
            && acteon_governance::permit::permit_revision_tag(&record.permits).is_ok()
            && record.submission_id
                == submission_id(&record.parent, &record.binding_digest, &record.message)
            && message_digest(&record.message).is_ok_and(|d| d == record.message_digest)
            && record.created_at_ms >= 0
            && state_valid
    }

    fn receipt(record: &SendRecord) -> PeerSendReceipt {
        let status = match &record.state {
            SendState::Accepted {
                task,
                source_context,
                ..
            } => PeerSendStatus::Accepted {
                task: task.clone(),
                source_context: source_context.clone(),
            },
            SendState::Rejected { code } => PeerSendStatus::Rejected { code: code.clone() },
            SendState::Registered | SendState::Delivering { .. } | SendState::Uncertain => {
                PeerSendStatus::Uncertain
            }
        };
        PeerSendReceipt {
            submission_id: record.submission_id,
            status,
        }
    }

    fn encode(&self, record: &SendRecord) -> Result<String, PeerTransportError> {
        let raw = serde_json::to_string(record).map_err(|_| PeerTransportError::Invalid)?;
        if raw.len() > MAX_RECORD_BYTES {
            return Err(PeerTransportError::Invalid);
        }
        self.dependencies
            .encryptor
            .as_ref()
            .map_or(Ok(raw.clone()), |encryptor| {
                encryptor
                    .encrypt_str(&raw)
                    .map_err(|_| PeerTransportError::Unavailable)
            })
    }

    fn encode_cancel(&self, record: &CancelRecord) -> Result<String, PeerTransportError> {
        let raw = serde_json::to_string(record).map_err(|_| PeerTransportError::Invalid)?;
        if raw.len() > MAX_RECORD_BYTES {
            return Err(PeerTransportError::Invalid);
        }
        self.dependencies
            .encryptor
            .as_ref()
            .map_or(Ok(raw.clone()), |encryptor| {
                encryptor
                    .encrypt_str(&raw)
                    .map_err(|_| PeerTransportError::Unavailable)
            })
    }

    fn encode_continuation(
        &self,
        record: &ContinuationRecord,
    ) -> Result<String, PeerTransportError> {
        let raw = serde_json::to_string(record).map_err(|_| PeerTransportError::Invalid)?;
        if raw.len() > MAX_RECORD_BYTES {
            return Err(PeerTransportError::Invalid);
        }
        self.dependencies
            .encryptor
            .as_ref()
            .map_or(Ok(raw.clone()), |encryptor| {
                encryptor
                    .encrypt_str(&raw)
                    .map_err(|_| PeerTransportError::Unavailable)
            })
    }

    fn decode(&self, raw: &str) -> Result<SendRecord, PeerTransportError> {
        if raw.len() > MAX_RECORD_BYTES * 2 {
            return Err(PeerTransportError::Conflict);
        }
        let decoded = self.dependencies.encryptor.as_ref().map_or_else(
            || Ok(raw.to_owned()),
            |encryptor| {
                encryptor
                    .decrypt_str(raw)
                    .map_err(|_| PeerTransportError::Unavailable)
            },
        )?;
        if decoded.len() > MAX_RECORD_BYTES {
            return Err(PeerTransportError::Conflict);
        }
        serde_json::from_str(&decoded).map_err(|_| PeerTransportError::Conflict)
    }

    fn decode_cancel(&self, raw: &str) -> Result<CancelRecord, PeerTransportError> {
        if raw.len() > MAX_RECORD_BYTES * 2 {
            return Err(PeerTransportError::Conflict);
        }
        let decoded = self.dependencies.encryptor.as_ref().map_or_else(
            || Ok(raw.to_owned()),
            |encryptor| {
                encryptor
                    .decrypt_str(raw)
                    .map_err(|_| PeerTransportError::Unavailable)
            },
        )?;
        if decoded.len() > MAX_RECORD_BYTES {
            return Err(PeerTransportError::Conflict);
        }
        serde_json::from_str(&decoded).map_err(|_| PeerTransportError::Conflict)
    }

    fn decode_continuation(&self, raw: &str) -> Result<ContinuationRecord, PeerTransportError> {
        if raw.len() > MAX_RECORD_BYTES * 2 {
            return Err(PeerTransportError::Conflict);
        }
        let decoded = self.dependencies.encryptor.as_ref().map_or_else(
            || Ok(raw.to_owned()),
            |encryptor| {
                encryptor
                    .decrypt_str(raw)
                    .map_err(|_| PeerTransportError::Unavailable)
            },
        )?;
        if decoded.len() > MAX_RECORD_BYTES {
            return Err(PeerTransportError::Conflict);
        }
        serde_json::from_str(&decoded).map_err(|_| PeerTransportError::Conflict)
    }

    async fn load_versioned(
        &self,
        key: &StateKey,
    ) -> Result<Option<(SendRecord, u64)>, PeerTransportError> {
        self.dependencies
            .state
            .get_versioned(key)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
            .map(|(raw, version)| self.decode(&raw).map(|record| (record, version)))
            .transpose()
    }

    async fn load_cancel_versioned(
        &self,
        key: &StateKey,
    ) -> Result<Option<(CancelRecord, u64)>, PeerTransportError> {
        self.dependencies
            .state
            .get_versioned(key)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
            .map(|(raw, version)| self.decode_cancel(&raw).map(|record| (record, version)))
            .transpose()
    }

    async fn load_continuation_versioned(
        &self,
        key: &StateKey,
    ) -> Result<Option<(ContinuationRecord, u64)>, PeerTransportError> {
        self.dependencies
            .state
            .get_versioned(key)
            .await
            .map_err(|_| PeerTransportError::Unavailable)?
            .map(|(raw, version)| {
                self.decode_continuation(&raw)
                    .map(|record| (record, version))
            })
            .transpose()
    }
}

fn state_can_reach(current: TaskState, observed: TaskState) -> bool {
    use TaskState::{
        AuthRequired, Canceled, Completed, Failed, InputRequired, Rejected, Submitted, Working,
    };
    current == observed
        || match current {
            Submitted => matches!(
                observed,
                Working | Completed | Failed | Canceled | InputRequired | AuthRequired | Rejected
            ),
            Working => matches!(
                observed,
                Completed | Failed | Canceled | InputRequired | AuthRequired
            ),
            InputRequired | AuthRequired => {
                matches!(
                    observed,
                    Working | Completed | Failed | Canceled | InputRequired | AuthRequired
                )
            }
            Completed | Failed | Canceled | Rejected => false,
        }
}

fn valid_task_progress(current: &Task, observed: &Task) -> bool {
    observed.validate().is_ok()
        && observed.id == current.id
        && observed.namespace == current.namespace
        && observed.tenant == current.tenant
        && observed.context_id == current.context_id
        && observed.created_at == current.created_at
        && observed.working_ttl_ms == current.working_ttl_ms
        && observed.updated_at >= current.updated_at
        && observed.status.timestamp >= current.status.timestamp
        && state_can_reach(current.status.state, observed.status.state)
        && (observed.updated_at != current.updated_at
            || serde_json::to_value(observed).ok() == serde_json::to_value(current).ok())
}
