//! Durable outbound A2A submission through one exact operator-qualified binding.
//!
//! The journal is stored in Acteon's configured [`StateStore`]. A durable intent
//! precedes the external call. Ambiguous delivery is never converted into a
//! rejection or an automatic resend. Explicit replay is available only when the
//! installed adapter qualifies identical-message submission as idempotent.

use super::{ApprovedPeerBinding, ApprovedPeerRegistry, PeerDiscoveryError};
use acteon_core::{ExecutionContextReference, Task, TaskMessage};
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
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;

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
    },
    Rejected {
        code: String,
    },
    Uncertain,
}

pub struct PeerSendRequest<'a> {
    pub endpoint: &'a str,
    pub transport: &'a str,
    pub parent: &'a ExecutionContextReference,
    pub permits: &'a [PermitReference],
    pub message: &'a TaskMessage,
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

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
                schema: 1,
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
            })) if task.validate().is_ok()
                && task.namespace == claimed.parent.namespace()
                && task.tenant == claimed.parent.tenant()
                && source_context == claimed.parent =>
            {
                SendState::Accepted {
                    task,
                    source_context,
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

    fn valid_record(&self, record: &SendRecord) -> bool {
        let state_valid = match &record.state {
            SendState::Registered | SendState::Delivering { .. } | SendState::Uncertain => true,
            SendState::Accepted {
                task,
                source_context,
            } => {
                task.validate().is_ok()
                    && task.namespace == record.parent.namespace()
                    && task.tenant == record.parent.tenant()
                    && source_context == &record.parent
            }
            SendState::Rejected { code } => valid_text(code),
        };
        record.schema == 1
            && record.binding_digest == self.adapter.binding_digest()
            && record.adapter_revision == self.adapter.revision()
            && record.capability == self.adapter.submission_capability()
            && valid_digest(&record.binding_digest)
            && valid_text(&record.adapter_revision)
            && valid_text(&record.endpoint)
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
}
