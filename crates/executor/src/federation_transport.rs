//! Durable outbound delivery for the Acteon federation protocol.
//!
//! The journal retains exact signed envelope and invocation bytes before any
//! network call. Import and invocation have separate checkpoints, so a crash
//! after a known import can continue without importing again. Ambiguous calls
//! stay explicit and are replayed only through an adapter that qualifies exact
//! requests as idempotent.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use acteon_core::{ExecutionContextReference, FederationImportReceipt, Task, TaskMessage};
use acteon_crypto::PayloadEncryptor;
use acteon_governance::{
    AuthorityStamp,
    federation::{SignedFederationEnvelope, SignedFederationInvocation},
};
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};
use acteon_time::Clock;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::federation_finality::{
    FederationFinalityCorrelation, FederationFinalityVerifier, FederationPeerFinality,
    MAX_FEDERATION_FINALITY_PROOF_BYTES,
};

pub const FEDERATION_DELIVERY_KIND: &str = "federation_outbound_delivery";
// V2 adds the mandatory local reconciliation-resource footprint. Reading a V1
// record as if it had an empty footprint would bypass management ceilings, so
// old records fail closed and require a reviewed migration.
const DELIVERY_SCHEMA: u32 = 2;
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENDPOINT_BYTES: usize = 2_048;
const MAX_CODE_BYTES: usize = 1_024;
const MAX_SIGNED_PAYLOAD_BYTES: usize = 65_536;

#[derive(Debug, Clone)]
pub enum FederationImportDisposition {
    Accepted(Box<FederationImportReceipt>),
    Rejected { code: String },
    Uncertain,
}

#[derive(Debug, Clone)]
pub enum FederationInvokeDisposition {
    Accepted(Box<Task>),
    Rejected { code: String },
    Uncertain,
}

pub struct FederationImportRequest<'a> {
    pub endpoint: &'a str,
    pub envelope: &'a SignedFederationEnvelope,
}

pub struct FederationInvokeRequest<'a> {
    pub endpoint: &'a str,
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub agent_id: &'a str,
    pub invocation: &'a SignedFederationInvocation,
    pub message: &'a TaskMessage,
}

/// Host-installed adapter. Credentials, endpoint policy and protocol
/// qualification stay inside the adapter and never enter the durable record.
#[async_trait]
pub trait FederationTransportAdapter: Send + Sync {
    fn revision(&self) -> &str;
    fn binding_digest(&self) -> &str;
    /// True only when the exact signed import and invocation can be resent and
    /// the remote implementation returns the original acceptance.
    fn exact_replay_idempotent(&self) -> bool;

    async fn import(
        &self,
        request: FederationImportRequest<'_>,
    ) -> Result<FederationImportDisposition, FederationTransportError>;

    async fn invoke(
        &self,
        request: FederationInvokeRequest<'_>,
    ) -> Result<FederationInvokeDisposition, FederationTransportError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationDeliveryStage {
    Import,
    Invoke,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationDeliveryStatus {
    Registered,
    ImportUncertain,
    Imported {
        receipt: Box<FederationImportReceipt>,
    },
    InvokeUncertain {
        receipt: Box<FederationImportReceipt>,
    },
    Accepted {
        receipt: Box<FederationImportReceipt>,
        task: Box<Task>,
    },
    Rejected {
        stage: FederationDeliveryStage,
        code: String,
    },
    /// Local recovery was stopped without asserting a remote outcome.
    Abandoned {
        stage: FederationDeliveryStage,
        abandonment: FederationDeliveryAbandonment,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDeliveryReceipt {
    pub delivery_id: Uuid,
    pub status: FederationDeliveryStatus,
}

/// Sanitized durable evidence for operator reconciliation. Signed payloads,
/// message bodies, credentials and endpoint URLs remain inside the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDeliveryEvidence {
    pub delivery_id: Uuid,
    pub parent_execution_id: Uuid,
    pub source: acteon_core::PrincipalIdentity,
    /// Local authority footprint retained at registration. API projections do
    /// not expose it, but management must cover it for every read or mutation.
    pub reconciliation_resources: Vec<acteon_core::ResourceRef>,
    pub agent_id: String,
    pub message_id: String,
    pub trust_id: String,
    pub trust_revision: u64,
    pub key_id: String,
    pub issuer_epoch: u64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub expired: bool,
    pub exact_replay_idempotent: bool,
    pub status: FederationDeliveryEvidenceStatus,
    /// Independently verified stage results, in protocol order. Payloads and
    /// peer task artifacts remain in the encrypted delivery journal.
    pub reconciliations: Vec<FederationDeliveryFinalityEvidence>,
}

/// Immutable operator audit attached to a locally abandoned ambiguous
/// delivery. It records no claim about whether the peer performed the effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDeliveryAbandonment {
    pub disposition_id: Uuid,
    pub operator: acteon_core::PrincipalIdentity,
    pub authority: AuthorityStamp,
    pub reason: String,
    pub recorded_at_ms: i64,
}

/// Authenticated local acceptance of one independently verified peer result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFinalityAcceptance {
    pub operator: acteon_core::PrincipalIdentity,
    pub authority: AuthorityStamp,
    pub accepted_at_ms: i64,
}

/// Sanitized proof metadata. The proof, receipt and task are retained inside
/// the encrypted journal and never returned by evidence-list operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDeliveryFinalityEvidence {
    pub stage: FederationDeliveryStage,
    pub accepted: bool,
    pub source_id: String,
    pub qualification_ref: String,
    pub verifier_revision: String,
    pub proof_digest: String,
    pub acceptance: FederationFinalityAcceptance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederationDeliveryEvidenceStatus {
    Registered,
    Importing {
        claim_expires_at_ms: i64,
    },
    ImportUncertain,
    Imported,
    Invoking {
        claim_expires_at_ms: i64,
    },
    InvokeUncertain,
    Accepted,
    Rejected {
        stage: FederationDeliveryStage,
        code: String,
    },
    Abandoned {
        stage: FederationDeliveryStage,
        abandonment: FederationDeliveryAbandonment,
    },
}

/// Exact trusted-host delivery input. The parent is correlation and local
/// lineage, not a bearer token. Callers must re-establish current source
/// authority before every `submit` or `replay_idempotent` call.
#[derive(Clone, Copy)]
pub struct FederationDeliveryIntent<'a> {
    pub parent: &'a ExecutionContextReference,
    pub reconciliation_resources: &'a [acteon_core::ResourceRef],
    pub endpoint: &'a str,
    pub agent_id: &'a str,
    pub envelope: &'a SignedFederationEnvelope,
    pub invocation: &'a SignedFederationInvocation,
    pub message: &'a TaskMessage,
}

pub struct FederationTransportDependencies {
    pub state: Arc<dyn StateStore>,
    pub clock: Arc<dyn Clock>,
    pub encryptor: Option<Arc<PayloadEncryptor>>,
}

pub struct DurableFederationTransport {
    dependencies: FederationTransportDependencies,
    adapter: Arc<dyn FederationTransportAdapter>,
    timeout: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum FederationTransportError {
    #[error("invalid outbound federation delivery")]
    Invalid,
    #[error("outbound federation delivery conflicts with durable intent")]
    Conflict,
    #[error("outbound federation delivery is not authorized for replay")]
    Refused,
    #[error("outbound federation delivery storage or adapter unavailable")]
    Unavailable,
}

/// Adapter-independent reader for retained outbound evidence. Old and
/// orphaned epochs remain visible after configuration changes, but the reader
/// has no credential or operation that can replay them.
pub struct FederationDeliveryEvidenceReader {
    state: Arc<dyn StateStore>,
    clock: Arc<dyn Clock>,
    encryptor: Option<Arc<PayloadEncryptor>>,
}

impl FederationDeliveryEvidenceReader {
    #[must_use]
    pub fn new(
        state: Arc<dyn StateStore>,
        clock: Arc<dyn Clock>,
        encryptor: Option<Arc<PayloadEncryptor>>,
    ) -> Self {
        Self {
            state,
            clock,
            encryptor,
        }
    }

    pub async fn list_evidence(
        &self,
        namespace: &str,
        tenant: &str,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<FederationDeliveryEvidence>, FederationTransportError> {
        if namespace.is_empty() || tenant.is_empty() || !(1..=500).contains(&limit) {
            return Err(FederationTransportError::Invalid);
        }
        let cursor = after.map(|delivery_id| {
            StateKey::new(
                namespace,
                tenant,
                KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
                delivery_id.to_string(),
            )
            .canonical()
        });
        let page = self
            .state
            .scan_keys_page(
                namespace,
                tenant,
                KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
                None,
                cursor.as_deref(),
                limit,
            )
            .await
            .map_err(|_| FederationTransportError::Unavailable)?;
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < 0 {
            return Err(FederationTransportError::Invalid);
        }
        let mut evidence = Vec::new();
        for (_, raw) in page.entries {
            let record = decode_evidence_record(self.encryptor.as_ref(), &raw)?;
            evidence.push(project_evidence(&record, now_ms)?);
        }
        Ok(evidence)
    }

    pub async fn get_evidence(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: Uuid,
    ) -> Result<Option<FederationDeliveryEvidence>, FederationTransportError> {
        if namespace.is_empty() || tenant.is_empty() || delivery_id.is_nil() {
            return Err(FederationTransportError::Invalid);
        }
        let key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
            delivery_id.to_string(),
        );
        let Some(raw) = self
            .state
            .get(&key)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
        else {
            return Ok(None);
        };
        let record = decode_evidence_record(self.encryptor.as_ref(), &raw)?;
        if record.delivery_id != delivery_id
            || record.parent.namespace() != namespace
            || record.parent.tenant() != tenant
        {
            return Err(FederationTransportError::Conflict);
        }
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < 0 {
            return Err(FederationTransportError::Invalid);
        }
        project_evidence(&record, now_ms).map(Some)
    }
}

/// Adapter-independent mutation boundary for operator reconciliation. It can
/// only stop local recovery of an already ambiguous delivery; it has no
/// endpoint, credential, signed request, or replay operation.
pub struct FederationDeliveryReconciler {
    state: Arc<dyn StateStore>,
    clock: Arc<dyn Clock>,
    encryptor: Option<Arc<PayloadEncryptor>>,
}

/// Trusted mutation boundary for peer finality. It owns no peer endpoint,
/// credential or replay adapter; public fields cannot select its verifiers.
pub struct FederationDeliveryFinalityReconciler {
    state: Arc<dyn StateStore>,
    clock: Arc<dyn Clock>,
    encryptor: Option<Arc<PayloadEncryptor>>,
    verifiers: BTreeMap<String, Arc<dyn FederationFinalityVerifier>>,
}

impl FederationDeliveryReconciler {
    #[must_use]
    pub fn new(
        state: Arc<dyn StateStore>,
        clock: Arc<dyn Clock>,
        encryptor: Option<Arc<PayloadEncryptor>>,
    ) -> Self {
        Self {
            state,
            clock,
            encryptor,
        }
    }

    /// Terminally stop local recovery while retaining the unknown peer
    /// outcome. The trusted host must derive `abandonment` from independently
    /// authenticated reconciliation authority immediately before this call.
    pub async fn abandon(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: Uuid,
        expected_source: &acteon_core::PrincipalIdentity,
        abandonment: FederationDeliveryAbandonment,
    ) -> Result<FederationDeliveryEvidence, FederationTransportError> {
        if namespace.is_empty()
            || tenant.is_empty()
            || delivery_id.is_nil()
            || !valid_abandonment(&abandonment)
        {
            return Err(FederationTransportError::Invalid);
        }
        let key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
            delivery_id.to_string(),
        );
        let (raw, version) = self
            .state
            .get_versioned(&key)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
            .ok_or(FederationTransportError::Conflict)?;
        let mut record = decode_evidence_record(self.encryptor.as_ref(), &raw)?;
        let envelope = record
            .envelope
            .envelope()
            .map_err(|_| FederationTransportError::Conflict)?;
        if record.delivery_id != delivery_id
            || record.parent.namespace() != namespace
            || record.parent.tenant() != tenant
            || &envelope.source != expected_source
            || abandonment.recorded_at_ms < record.created_at_ms
        {
            return Err(FederationTransportError::Conflict);
        }
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < 0 || abandonment.recorded_at_ms > now_ms {
            return Err(FederationTransportError::Invalid);
        }
        let (stage, receipt) = match &record.state {
            DeliveryState::ImportUncertain => (FederationDeliveryStage::Import, None),
            DeliveryState::InvokeUncertain { receipt } => {
                (FederationDeliveryStage::Invoke, Some(receipt.clone()))
            }
            DeliveryState::Importing {
                claim_expires_at_ms,
                ..
            } if now_ms >= *claim_expires_at_ms => (FederationDeliveryStage::Import, None),
            DeliveryState::Invoking {
                receipt,
                claim_expires_at_ms,
                ..
            } if now_ms >= *claim_expires_at_ms => {
                (FederationDeliveryStage::Invoke, Some(receipt.clone()))
            }
            DeliveryState::Abandoned {
                abandonment: existing,
                ..
            } if existing == &abandonment => return project_evidence(&record, now_ms),
            _ => return Err(FederationTransportError::Conflict),
        };
        record.state = DeliveryState::Abandoned {
            stage,
            receipt,
            abandonment,
        };
        match self
            .state
            .compare_and_swap(
                &key,
                version,
                &encode_evidence_record(self.encryptor.as_ref(), &record)?,
                None,
            )
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
        {
            CasResult::Ok => project_evidence(&record, now_ms),
            CasResult::Conflict { .. } => Err(FederationTransportError::Conflict),
        }
    }
}

impl FederationDeliveryFinalityReconciler {
    /// Install a bounded verifier set keyed by the immutable outbound binding
    /// digest. Empty installations are rejected so this boundary cannot appear
    /// enabled while accepting no qualified source.
    pub fn new_trusted(
        state: Arc<dyn StateStore>,
        clock: Arc<dyn Clock>,
        encryptor: Option<Arc<PayloadEncryptor>>,
        verifiers: BTreeMap<String, Arc<dyn FederationFinalityVerifier>>,
    ) -> Result<Self, FederationTransportError> {
        if verifiers.is_empty()
            || verifiers.len() > 128
            || verifiers.iter().any(|(binding, verifier)| {
                !valid_digest(binding)
                    || !valid_text(verifier.source_id(), 255)
                    || !valid_text(verifier.qualification_ref(), 255)
                    || !valid_text(verifier.revision(), 120)
            })
        {
            return Err(FederationTransportError::Invalid);
        }
        Ok(Self {
            state,
            clock,
            encryptor,
            verifiers,
        })
    }

    /// Return correlation only for an ambiguous stage (or an expired active
    /// claim). Known results and stages that never crossed the boundary are not
    /// eligible for external finality.
    pub async fn correlation(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: Uuid,
        expected_source: &acteon_core::PrincipalIdentity,
    ) -> Result<FederationFinalityCorrelation, FederationTransportError> {
        let (record, _) = self
            .load(namespace, tenant, delivery_id, expected_source)
            .await?;
        self.correlation_for_record(&record)
    }

    /// Verify and durably attach one stage result. This method never calls the
    /// peer. Exact proof replay is idempotent; a different result for the same
    /// stage conflicts.
    pub async fn reconcile(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: Uuid,
        expected_source: &acteon_core::PrincipalIdentity,
        proof: &[u8],
        acceptance: FederationFinalityAcceptance,
    ) -> Result<FederationDeliveryEvidence, FederationTransportError> {
        if proof.is_empty()
            || proof.len() > MAX_FEDERATION_FINALITY_PROOF_BYTES
            || !valid_finality_acceptance(&acceptance)
        {
            return Err(FederationTransportError::Invalid);
        }
        let proof_digest = format!("{:x}", Sha256::digest(proof));
        for _ in 0..3 {
            let (mut record, version) = self
                .load(namespace, tenant, delivery_id, expected_source)
                .await?;
            let now_ms = self.now_ms()?;
            if acceptance.accepted_at_ms < record.created_at_ms
                || acceptance.accepted_at_ms > now_ms
            {
                return Err(FederationTransportError::Invalid);
            }
            let verifier = self
                .verifiers
                .get(&record.binding_digest)
                .ok_or(FederationTransportError::Refused)?;
            if let Some(existing) = record
                .reconciliations
                .iter()
                .find(|entry| entry.proof_digest == proof_digest)
            {
                let accepted_finality = verifier.verify(&existing.correlation, proof)?;
                if existing.source_id != verifier.source_id()
                    || existing.qualification_ref != verifier.qualification_ref()
                    || existing.verifier_revision != verifier.revision()
                    || !same_peer_finality(&existing.finality, &accepted_finality)?
                {
                    return Err(FederationTransportError::Conflict);
                }
                return project_evidence(&record, self.now_ms()?);
            }
            let correlation = self.correlation_for_record(&record)?;
            if record
                .reconciliations
                .iter()
                .any(|entry| entry.correlation.stage == correlation.stage)
            {
                return Err(FederationTransportError::Conflict);
            }
            let finality = verifier.verify(&correlation, proof)?;
            validate_peer_finality(&record, &correlation, &finality)?;
            apply_peer_finality(&mut record, &finality);
            record
                .reconciliations
                .push(FederationDeliveryReconciliation {
                    correlation,
                    finality,
                    source_id: verifier.source_id().into(),
                    qualification_ref: verifier.qualification_ref().into(),
                    verifier_revision: verifier.revision().into(),
                    proof: proof.to_vec(),
                    proof_digest: proof_digest.clone(),
                    acceptance: acceptance.clone(),
                });
            let key = delivery_key(namespace, tenant, delivery_id);
            match self
                .state
                .compare_and_swap(
                    &key,
                    version,
                    &encode_evidence_record(self.encryptor.as_ref(), &record)?,
                    None,
                )
                .await
                .map_err(|_| FederationTransportError::Unavailable)?
            {
                CasResult::Ok => return project_evidence(&record, now_ms),
                CasResult::Conflict { .. } => {}
            }
        }
        Err(FederationTransportError::Unavailable)
    }

    async fn load(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: Uuid,
        expected_source: &acteon_core::PrincipalIdentity,
    ) -> Result<(DeliveryRecord, u64), FederationTransportError> {
        if namespace.is_empty() || tenant.is_empty() || delivery_id.is_nil() {
            return Err(FederationTransportError::Invalid);
        }
        let key = delivery_key(namespace, tenant, delivery_id);
        let (raw, version) = self
            .state
            .get_versioned(&key)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
            .ok_or(FederationTransportError::Conflict)?;
        let record = decode_evidence_record(self.encryptor.as_ref(), &raw)?;
        let envelope = record
            .envelope
            .envelope()
            .map_err(|_| FederationTransportError::Conflict)?;
        if record.delivery_id != delivery_id
            || record.parent.namespace() != namespace
            || record.parent.tenant() != tenant
            || &envelope.source != expected_source
        {
            return Err(FederationTransportError::Conflict);
        }
        Ok((record, version))
    }

    fn correlation_for_record(
        &self,
        record: &DeliveryRecord,
    ) -> Result<FederationFinalityCorrelation, FederationTransportError> {
        let now_ms = self.now_ms()?;
        let stage = match &record.state {
            DeliveryState::ImportUncertain => FederationDeliveryStage::Import,
            DeliveryState::Importing {
                claim_expires_at_ms,
                ..
            } if now_ms >= *claim_expires_at_ms => FederationDeliveryStage::Import,
            DeliveryState::InvokeUncertain { .. } => FederationDeliveryStage::Invoke,
            DeliveryState::Invoking {
                claim_expires_at_ms,
                ..
            } if now_ms >= *claim_expires_at_ms => FederationDeliveryStage::Invoke,
            _ => return Err(FederationTransportError::Conflict),
        };
        finality_correlation(record, stage)
    }

    fn now_ms(&self) -> Result<i64, FederationTransportError> {
        let now_ms = self.clock.now().timestamp_millis();
        (now_ms >= 0)
            .then_some(now_ms)
            .ok_or(FederationTransportError::Invalid)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum DeliveryState {
    Registered,
    Importing {
        claim_id: Uuid,
        claim_expires_at_ms: i64,
    },
    ImportUncertain,
    Imported {
        receipt: Box<FederationImportReceipt>,
    },
    Invoking {
        receipt: Box<FederationImportReceipt>,
        claim_id: Uuid,
        claim_expires_at_ms: i64,
    },
    InvokeUncertain {
        receipt: Box<FederationImportReceipt>,
    },
    Accepted {
        receipt: Box<FederationImportReceipt>,
        task: Box<Task>,
    },
    Rejected {
        stage: FederationDeliveryStage,
        code: String,
    },
    Abandoned {
        stage: FederationDeliveryStage,
        /// Retained only to prove invoke-stage lineage. The evidence projection
        /// never exposes the import receipt.
        receipt: Option<Box<FederationImportReceipt>>,
        abandonment: FederationDeliveryAbandonment,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryRecord {
    schema: u32,
    delivery_id: Uuid,
    adapter_revision: String,
    binding_digest: String,
    exact_replay_idempotent: bool,
    parent: ExecutionContextReference,
    reconciliation_resources: Vec<acteon_core::ResourceRef>,
    endpoint: String,
    agent_id: String,
    envelope: SignedFederationEnvelope,
    invocation: SignedFederationInvocation,
    message: TaskMessage,
    created_at_ms: i64,
    state: DeliveryState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reconciliations: Vec<FederationDeliveryReconciliation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationDeliveryReconciliation {
    correlation: FederationFinalityCorrelation,
    finality: FederationPeerFinality,
    source_id: String,
    qualification_ref: String,
    verifier_revision: String,
    proof: Vec<u8>,
    proof_digest: String,
    acceptance: FederationFinalityAcceptance,
}

impl DurableFederationTransport {
    pub fn new_trusted(
        dependencies: FederationTransportDependencies,
        adapter: Arc<dyn FederationTransportAdapter>,
        timeout: Duration,
    ) -> Result<Self, FederationTransportError> {
        if timeout.is_zero()
            || !valid_text(adapter.revision(), 120)
            || !valid_digest(adapter.binding_digest())
        {
            return Err(FederationTransportError::Invalid);
        }
        Ok(Self {
            dependencies,
            adapter,
            timeout,
        })
    }

    /// List this adapter binding's retained delivery evidence in stable ID
    /// order. The scan is scope-local and the result is strictly bounded.
    pub async fn list_evidence(
        &self,
        namespace: &str,
        tenant: &str,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<FederationDeliveryEvidence>, FederationTransportError> {
        if namespace.is_empty() || tenant.is_empty() || !(1..=500).contains(&limit) {
            return Err(FederationTransportError::Invalid);
        }
        let mut records = self
            .dependencies
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
                None,
            )
            .await
            .map_err(|_| FederationTransportError::Unavailable)?;
        records.sort_by(|left, right| left.0.cmp(&right.0));
        let mut evidence = Vec::new();
        for (_, raw) in records {
            let Some(record) = self.decode_matching(&raw)? else {
                continue;
            };
            if after.is_some_and(|cursor| record.delivery_id <= cursor) {
                continue;
            }
            evidence.push(self.evidence(&record)?);
            if evidence.len() == limit {
                break;
            }
        }
        Ok(evidence)
    }

    /// Persist exact signed bytes before importing or invoking. Repeated calls
    /// never resend an ambiguous stage. A known import may safely advance to
    /// invocation because the invocation has not yet been delivered.
    pub async fn submit(
        &self,
        intent: FederationDeliveryIntent<'_>,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        self.register(intent).await?;
        self.deliver_registered(intent).await
    }

    /// Retain the exact intent without crossing the network boundary. A host
    /// can then atomically reserve current local authority before calling
    /// `deliver_registered` with the same intent.
    pub async fn register(
        &self,
        intent: FederationDeliveryIntent<'_>,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let expected = self.prepare(intent)?;
        let key = Self::key(&expected);
        self.dependencies
            .state
            .check_and_set(&key, &self.encode(&expected)?, None)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?;
        self.observe_record(&key, &expected).await
    }

    /// Cross the network only after a trusted host has established any local
    /// permit, budget and closure reservation required for this exact intent.
    /// Missing durable intent is a conflict and no adapter call is made.
    pub async fn deliver_registered(
        &self,
        intent: FederationDeliveryIntent<'_>,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let expected = self.prepare(intent)?;
        let key = Self::key(&expected);
        if self
            .dependencies
            .state
            .get(&key)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
            .is_none()
        {
            return Err(FederationTransportError::Conflict);
        }
        for _ in 0..3 {
            let receipt = self.advance(&key, &expected, false).await?;
            if !matches!(receipt.status, FederationDeliveryStatus::Imported { .. }) {
                return Ok(receipt);
            }
        }
        Err(FederationTransportError::Unavailable)
    }

    /// Record a definitive host-side refusal while no call for the current
    /// stage is in flight or ambiguous. This cannot overwrite uncertain or
    /// accepted remote evidence.
    pub async fn refuse_registered(
        &self,
        intent: FederationDeliveryIntent<'_>,
        code: &str,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        if !valid_code(code) {
            return Err(FederationTransportError::Invalid);
        }
        let expected = self.prepare(intent)?;
        let key = Self::key(&expected);
        let (current, version) = self
            .load_versioned(&key)
            .await?
            .ok_or(FederationTransportError::Conflict)?;
        self.same_intent(&current, &expected)?;
        let stage = match &current.state {
            DeliveryState::Registered => FederationDeliveryStage::Import,
            DeliveryState::Imported { .. } => FederationDeliveryStage::Invoke,
            DeliveryState::Accepted { .. }
            | DeliveryState::Rejected { .. }
            | DeliveryState::Abandoned { .. } => {
                return Ok(Self::receipt(&current));
            }
            DeliveryState::Importing { .. }
            | DeliveryState::ImportUncertain
            | DeliveryState::Invoking { .. }
            | DeliveryState::InvokeUncertain { .. } => {
                return Err(FederationTransportError::Conflict);
            }
        };
        self.reject_without_delivery(&key, current, version, stage, code)
            .await
    }

    /// Inspect the exact durable intent without issuing network traffic.
    pub async fn observe(
        &self,
        intent: FederationDeliveryIntent<'_>,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let expected = self.prepare(intent)?;
        self.observe_record(&Self::key(&expected), &expected).await
    }

    /// Recover an ambiguous or expired claimed stage only for an adapter whose
    /// exact protocol binding is qualified as replay-idempotent.
    pub async fn replay_idempotent(
        &self,
        intent: FederationDeliveryIntent<'_>,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        if !self.adapter.exact_replay_idempotent() {
            return Err(FederationTransportError::Refused);
        }
        let expected = self.prepare(intent)?;
        let key = Self::key(&expected);
        if self
            .dependencies
            .state
            .get(&key)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
            .is_none()
        {
            return Err(FederationTransportError::Conflict);
        }
        for _ in 0..3 {
            let receipt = self.advance(&key, &expected, true).await?;
            if !matches!(receipt.status, FederationDeliveryStatus::Imported { .. }) {
                return Ok(receipt);
            }
        }
        Err(FederationTransportError::Unavailable)
    }

    async fn advance(
        &self,
        key: &StateKey,
        expected: &DeliveryRecord,
        replay: bool,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let (record, version) = self
            .load_versioned(key)
            .await?
            .ok_or(FederationTransportError::Unavailable)?;
        self.same_intent(&record, expected)?;
        match &record.state {
            DeliveryState::Registered => self.claim_import(key, record, version).await,
            DeliveryState::Imported { .. } => self.claim_invoke(key, record, version).await,
            DeliveryState::ImportUncertain if replay => {
                self.claim_import(key, record, version).await
            }
            DeliveryState::InvokeUncertain { .. } if replay => {
                self.claim_invoke(key, record, version).await
            }
            DeliveryState::Importing {
                claim_expires_at_ms,
                ..
            } if replay && self.claim_expired(*claim_expires_at_ms) => {
                self.claim_import(key, record, version).await
            }
            DeliveryState::Invoking {
                claim_expires_at_ms,
                ..
            } if replay && self.claim_expired(*claim_expires_at_ms) => {
                self.claim_invoke(key, record, version).await
            }
            _ => Ok(Self::receipt(&record)),
        }
    }

    async fn claim_import(
        &self,
        key: &StateKey,
        current: DeliveryRecord,
        version: u64,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let now_ms = self.now_ms()?;
        let envelope = current
            .envelope
            .envelope()
            .map_err(|_| FederationTransportError::Conflict)?;
        if now_ms < envelope.issued_at_ms {
            return Err(FederationTransportError::Refused);
        }
        if now_ms >= envelope.expires_at_ms {
            return if matches!(&current.state, DeliveryState::Registered) {
                self.reject_without_delivery(
                    key,
                    current,
                    version,
                    FederationDeliveryStage::Import,
                    "signed_request_expired",
                )
                .await
            } else {
                Err(FederationTransportError::Refused)
            };
        }
        let mut claimed = current.clone();
        claimed.state = DeliveryState::Importing {
            claim_id: Uuid::new_v4(),
            claim_expires_at_ms: self.claim_deadline(now_ms),
        };
        if !matches!(
            self.dependencies
                .state
                .compare_and_swap(key, version, &self.encode(&claimed)?, None)
                .await
                .map_err(|_| FederationTransportError::Unavailable)?,
            CasResult::Ok
        ) {
            return self.observe_record(key, &current).await;
        }
        let disposition = acteon_time::timeout(
            self.dependencies.clock.as_ref(),
            self.timeout,
            self.adapter.import(FederationImportRequest {
                endpoint: &claimed.endpoint,
                envelope: &claimed.envelope,
            }),
        )
        .await;
        let state = match disposition {
            Ok(Ok(FederationImportDisposition::Accepted(receipt)))
                if receipt_matches(&claimed, &receipt) =>
            {
                DeliveryState::Imported { receipt }
            }
            Ok(Ok(FederationImportDisposition::Rejected { code })) if valid_code(&code) => {
                DeliveryState::Rejected {
                    stage: FederationDeliveryStage::Import,
                    code,
                }
            }
            _ => DeliveryState::ImportUncertain,
        };
        self.settle(key, &current, claimed, version + 1, state)
            .await
    }

    async fn claim_invoke(
        &self,
        key: &StateKey,
        current: DeliveryRecord,
        version: u64,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let receipt = match &current.state {
            DeliveryState::Imported { receipt }
            | DeliveryState::InvokeUncertain { receipt }
            | DeliveryState::Invoking { receipt, .. } => receipt.clone(),
            _ => return Err(FederationTransportError::Conflict),
        };
        let now_ms = self.now_ms()?;
        let invocation = current
            .invocation
            .invocation()
            .map_err(|_| FederationTransportError::Conflict)?;
        let envelope = current
            .envelope
            .envelope()
            .map_err(|_| FederationTransportError::Conflict)?;
        if now_ms < invocation.issued_at_ms {
            return Err(FederationTransportError::Refused);
        }
        if now_ms >= invocation.expires_at_ms {
            return if matches!(&current.state, DeliveryState::Imported { .. }) {
                self.reject_without_delivery(
                    key,
                    current,
                    version,
                    FederationDeliveryStage::Invoke,
                    "signed_request_expired",
                )
                .await
            } else {
                Err(FederationTransportError::Refused)
            };
        }
        let mut claimed = current.clone();
        claimed.state = DeliveryState::Invoking {
            receipt: receipt.clone(),
            claim_id: Uuid::new_v4(),
            claim_expires_at_ms: self.claim_deadline(now_ms),
        };
        if !matches!(
            self.dependencies
                .state
                .compare_and_swap(key, version, &self.encode(&claimed)?, None)
                .await
                .map_err(|_| FederationTransportError::Unavailable)?,
            CasResult::Ok
        ) {
            return self.observe_record(key, &current).await;
        }
        let disposition = acteon_time::timeout(
            self.dependencies.clock.as_ref(),
            self.timeout,
            self.adapter.invoke(FederationInvokeRequest {
                endpoint: &claimed.endpoint,
                namespace: envelope.agent_resource.namespace(),
                tenant: envelope.agent_resource.tenant(),
                agent_id: &claimed.agent_id,
                invocation: &claimed.invocation,
                message: &claimed.message,
            }),
        )
        .await;
        let state = match disposition {
            Ok(Ok(FederationInvokeDisposition::Accepted(task)))
                if task_matches(&claimed, &task) =>
            {
                DeliveryState::Accepted { receipt, task }
            }
            Ok(Ok(FederationInvokeDisposition::Rejected { code })) if valid_code(&code) => {
                DeliveryState::Rejected {
                    stage: FederationDeliveryStage::Invoke,
                    code,
                }
            }
            _ => DeliveryState::InvokeUncertain { receipt },
        };
        self.settle(key, &current, claimed, version + 1, state)
            .await
    }

    async fn settle(
        &self,
        key: &StateKey,
        expected: &DeliveryRecord,
        mut claimed: DeliveryRecord,
        claimed_version: u64,
        state: DeliveryState,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        claimed.state = state;
        match self
            .dependencies
            .state
            .compare_and_swap(key, claimed_version, &self.encode(&claimed)?, None)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
        {
            CasResult::Ok => Ok(Self::receipt(&claimed)),
            CasResult::Conflict { .. } => self.observe_record(key, expected).await,
        }
    }

    async fn reject_without_delivery(
        &self,
        key: &StateKey,
        mut current: DeliveryRecord,
        version: u64,
        stage: FederationDeliveryStage,
        code: &str,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        current.state = DeliveryState::Rejected {
            stage,
            code: code.into(),
        };
        match self
            .dependencies
            .state
            .compare_and_swap(key, version, &self.encode(&current)?, None)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
        {
            CasResult::Ok => Ok(Self::receipt(&current)),
            CasResult::Conflict { .. } => self.observe_record(key, &current).await,
        }
    }

    fn prepare(
        &self,
        intent: FederationDeliveryIntent<'_>,
    ) -> Result<DeliveryRecord, FederationTransportError> {
        if !valid_endpoint(intent.endpoint)
            || !valid_text(intent.agent_id, 120)
            || intent.message.task_id.is_some()
            || intent.message.validate().is_err()
            || intent.envelope.payload.len() > MAX_SIGNED_PAYLOAD_BYTES
            || intent.invocation.payload.len() > MAX_SIGNED_PAYLOAD_BYTES
            || !valid_signature(&intent.envelope.signature)
            || !valid_signature(&intent.invocation.signature)
            || intent.reconciliation_resources.is_empty()
            || intent.reconciliation_resources.len() > 16
            || intent
                .reconciliation_resources
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != intent.reconciliation_resources.len()
            || intent.reconciliation_resources.iter().any(|resource| {
                resource.namespace() != intent.parent.namespace()
                    || resource.tenant() != intent.parent.tenant()
            })
        {
            return Err(FederationTransportError::Invalid);
        }
        let envelope = intent
            .envelope
            .envelope()
            .map_err(|_| FederationTransportError::Invalid)?;
        let invocation = intent
            .invocation
            .invocation()
            .map_err(|_| FederationTransportError::Invalid)?;
        let envelope_digest = intent
            .envelope
            .digest()
            .map_err(|_| FederationTransportError::Invalid)?;
        if invocation.import.id != envelope.id
            || invocation.import.envelope_digest != envelope_digest
            || envelope.agent_resource.id() != intent.agent_id
            || intent.parent.principal() != &envelope.source
            || !same_message(&invocation.message, intent.message)?
        {
            return Err(FederationTransportError::Conflict);
        }
        let created_at_ms = self.now_ms()?;
        let delivery_id = delivery_id(intent.parent, intent.invocation)?;
        Ok(DeliveryRecord {
            schema: DELIVERY_SCHEMA,
            delivery_id,
            adapter_revision: self.adapter.revision().into(),
            binding_digest: self.adapter.binding_digest().into(),
            exact_replay_idempotent: self.adapter.exact_replay_idempotent(),
            parent: intent.parent.clone(),
            reconciliation_resources: intent.reconciliation_resources.to_vec(),
            endpoint: intent.endpoint.into(),
            agent_id: intent.agent_id.into(),
            envelope: intent.envelope.clone(),
            invocation: intent.invocation.clone(),
            message: intent.message.clone(),
            created_at_ms,
            state: DeliveryState::Registered,
            reconciliations: Vec::new(),
        })
    }

    async fn observe_record(
        &self,
        key: &StateKey,
        expected: &DeliveryRecord,
    ) -> Result<FederationDeliveryReceipt, FederationTransportError> {
        let (record, _) = self
            .load_versioned(key)
            .await?
            .ok_or(FederationTransportError::Conflict)?;
        self.same_intent(&record, expected)?;
        Ok(Self::receipt(&record))
    }

    fn same_intent(
        &self,
        actual: &DeliveryRecord,
        expected: &DeliveryRecord,
    ) -> Result<(), FederationTransportError> {
        if !self.valid_record(actual) || !self.valid_record(expected) {
            return Err(FederationTransportError::Conflict);
        }
        if actual.delivery_id != expected.delivery_id
            || actual.adapter_revision != expected.adapter_revision
            || actual.binding_digest != expected.binding_digest
            || actual.exact_replay_idempotent != expected.exact_replay_idempotent
            || actual.parent != expected.parent
            || actual.reconciliation_resources != expected.reconciliation_resources
            || actual.endpoint != expected.endpoint
            || actual.agent_id != expected.agent_id
            || actual.envelope != expected.envelope
            || actual.invocation != expected.invocation
            || !same_message(&actual.message, &expected.message)?
        {
            return Err(FederationTransportError::Conflict);
        }
        Ok(())
    }

    fn valid_record(&self, record: &DeliveryRecord) -> bool {
        if record.adapter_revision != self.adapter.revision()
            || record.binding_digest != self.adapter.binding_digest()
            || record.exact_replay_idempotent != self.adapter.exact_replay_idempotent()
            || !valid_evidence_record(record)
        {
            return false;
        }
        true
    }

    fn receipt(record: &DeliveryRecord) -> FederationDeliveryReceipt {
        let status = match &record.state {
            DeliveryState::Registered => FederationDeliveryStatus::Registered,
            DeliveryState::Importing { .. } | DeliveryState::ImportUncertain => {
                FederationDeliveryStatus::ImportUncertain
            }
            DeliveryState::Imported { receipt } => FederationDeliveryStatus::Imported {
                receipt: receipt.clone(),
            },
            DeliveryState::Invoking { receipt, .. }
            | DeliveryState::InvokeUncertain { receipt } => {
                FederationDeliveryStatus::InvokeUncertain {
                    receipt: receipt.clone(),
                }
            }
            DeliveryState::Accepted { receipt, task } => FederationDeliveryStatus::Accepted {
                receipt: receipt.clone(),
                task: task.clone(),
            },
            DeliveryState::Rejected { stage, code } => FederationDeliveryStatus::Rejected {
                stage: *stage,
                code: code.clone(),
            },
            DeliveryState::Abandoned {
                stage, abandonment, ..
            } => FederationDeliveryStatus::Abandoned {
                stage: *stage,
                abandonment: abandonment.clone(),
            },
        };
        FederationDeliveryReceipt {
            delivery_id: record.delivery_id,
            status,
        }
    }

    fn key(record: &DeliveryRecord) -> StateKey {
        delivery_key(
            record.parent.namespace(),
            record.parent.tenant(),
            record.delivery_id,
        )
    }

    fn now_ms(&self) -> Result<i64, FederationTransportError> {
        let now_ms = self.dependencies.clock.now().timestamp_millis();
        if now_ms < 0 {
            return Err(FederationTransportError::Invalid);
        }
        Ok(now_ms)
    }

    fn claim_deadline(&self, now_ms: i64) -> i64 {
        i64::try_from(self.timeout.as_millis())
            .ok()
            .and_then(|timeout| now_ms.checked_add(timeout))
            .unwrap_or(i64::MAX)
    }

    fn claim_expired(&self, deadline_ms: i64) -> bool {
        self.dependencies.clock.now().timestamp_millis() >= deadline_ms
    }

    fn encode(&self, record: &DeliveryRecord) -> Result<String, FederationTransportError> {
        let raw = serde_json::to_string(record).map_err(|_| FederationTransportError::Invalid)?;
        if raw.len() > MAX_RECORD_BYTES {
            return Err(FederationTransportError::Invalid);
        }
        self.dependencies
            .encryptor
            .as_ref()
            .map_or(Ok(raw.clone()), |encryptor| {
                encryptor
                    .encrypt_str(&raw)
                    .map_err(|_| FederationTransportError::Unavailable)
            })
    }

    fn decode(&self, raw: &str) -> Result<DeliveryRecord, FederationTransportError> {
        if raw.len() > MAX_RECORD_BYTES * 2 {
            return Err(FederationTransportError::Conflict);
        }
        let plain = self.dependencies.encryptor.as_ref().map_or_else(
            || Ok(raw.to_owned()),
            |encryptor| {
                encryptor
                    .decrypt_str(raw)
                    .map_err(|_| FederationTransportError::Unavailable)
            },
        )?;
        if plain.len() > MAX_RECORD_BYTES {
            return Err(FederationTransportError::Conflict);
        }
        let record =
            serde_json::from_str(&plain).map_err(|_| FederationTransportError::Conflict)?;
        if !self.valid_record(&record) {
            return Err(FederationTransportError::Conflict);
        }
        Ok(record)
    }

    fn decode_matching(
        &self,
        raw: &str,
    ) -> Result<Option<DeliveryRecord>, FederationTransportError> {
        if raw.len() > MAX_RECORD_BYTES * 2 {
            return Err(FederationTransportError::Conflict);
        }
        let plain = self.dependencies.encryptor.as_ref().map_or_else(
            || Ok(raw.to_owned()),
            |encryptor| {
                encryptor
                    .decrypt_str(raw)
                    .map_err(|_| FederationTransportError::Unavailable)
            },
        )?;
        if plain.len() > MAX_RECORD_BYTES {
            return Err(FederationTransportError::Conflict);
        }
        let record: DeliveryRecord =
            serde_json::from_str(&plain).map_err(|_| FederationTransportError::Conflict)?;
        if record.adapter_revision != self.adapter.revision()
            || record.binding_digest != self.adapter.binding_digest()
        {
            return Ok(None);
        }
        if !self.valid_record(&record) {
            return Err(FederationTransportError::Conflict);
        }
        Ok(Some(record))
    }

    fn evidence(
        &self,
        record: &DeliveryRecord,
    ) -> Result<FederationDeliveryEvidence, FederationTransportError> {
        project_evidence(record, self.now_ms()?)
    }

    async fn load_versioned(
        &self,
        key: &StateKey,
    ) -> Result<Option<(DeliveryRecord, u64)>, FederationTransportError> {
        self.dependencies
            .state
            .get_versioned(key)
            .await
            .map_err(|_| FederationTransportError::Unavailable)?
            .map(|(raw, version)| self.decode(&raw).map(|record| (record, version)))
            .transpose()
    }
}

fn decode_evidence_record(
    encryptor: Option<&Arc<PayloadEncryptor>>,
    raw: &str,
) -> Result<DeliveryRecord, FederationTransportError> {
    if raw.len() > MAX_RECORD_BYTES * 2 {
        return Err(FederationTransportError::Conflict);
    }
    let plain = encryptor.map_or_else(
        || Ok(raw.to_owned()),
        |encryptor| {
            encryptor
                .decrypt_str(raw)
                .map_err(|_| FederationTransportError::Unavailable)
        },
    )?;
    if plain.len() > MAX_RECORD_BYTES {
        return Err(FederationTransportError::Conflict);
    }
    let record: DeliveryRecord =
        serde_json::from_str(&plain).map_err(|_| FederationTransportError::Conflict)?;
    if !valid_evidence_record(&record) {
        return Err(FederationTransportError::Conflict);
    }
    Ok(record)
}

fn encode_evidence_record(
    encryptor: Option<&Arc<PayloadEncryptor>>,
    record: &DeliveryRecord,
) -> Result<String, FederationTransportError> {
    let raw = serde_json::to_string(record).map_err(|_| FederationTransportError::Invalid)?;
    if raw.len() > MAX_RECORD_BYTES {
        return Err(FederationTransportError::Invalid);
    }
    encryptor.map_or(Ok(raw.clone()), |encryptor| {
        encryptor
            .encrypt_str(&raw)
            .map_err(|_| FederationTransportError::Unavailable)
    })
}

fn delivery_key(namespace: &str, tenant: &str, delivery_id: Uuid) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
        delivery_id.to_string(),
    )
}

fn finality_correlation(
    record: &DeliveryRecord,
    stage: FederationDeliveryStage,
) -> Result<FederationFinalityCorrelation, FederationTransportError> {
    let envelope = record
        .envelope
        .envelope()
        .map_err(|_| FederationTransportError::Conflict)?;
    let invocation = record
        .invocation
        .invocation()
        .map_err(|_| FederationTransportError::Conflict)?;
    Ok(FederationFinalityCorrelation {
        delivery_id: record.delivery_id,
        stage,
        parent_execution_id: record.parent.execution_id(),
        source: envelope.source,
        adapter_revision: record.adapter_revision.clone(),
        binding_digest: record.binding_digest.clone(),
        envelope_digest: record
            .envelope
            .digest()
            .map_err(|_| FederationTransportError::Conflict)?,
        invocation_digest: record
            .invocation
            .digest()
            .map_err(|_| FederationTransportError::Conflict)?,
        import_id: envelope.id,
        invocation_nonce: invocation.nonce,
        agent_id: record.agent_id.clone(),
        message_id: invocation.message.message_id,
    })
}

fn validate_peer_finality(
    record: &DeliveryRecord,
    correlation: &FederationFinalityCorrelation,
    finality: &FederationPeerFinality,
) -> Result<(), FederationTransportError> {
    if finality.stage() != correlation.stage {
        return Err(FederationTransportError::Conflict);
    }
    match finality {
        FederationPeerFinality::ImportAccepted { receipt } => receipt_matches(record, receipt)
            .then_some(())
            .ok_or(FederationTransportError::Conflict),
        FederationPeerFinality::ImportRejected { code } => valid_code(code)
            .then_some(())
            .ok_or(FederationTransportError::Invalid),
        FederationPeerFinality::InvocationAccepted { receipt, task } => {
            (receipt_matches(record, receipt) && task_matches(record, task))
                .then_some(())
                .ok_or(FederationTransportError::Conflict)
        }
        FederationPeerFinality::InvocationRejected { receipt, code } => {
            (receipt_matches(record, receipt) && valid_code(code))
                .then_some(())
                .ok_or(FederationTransportError::Conflict)
        }
    }
}

fn apply_peer_finality(record: &mut DeliveryRecord, finality: &FederationPeerFinality) {
    record.state = match finality {
        FederationPeerFinality::ImportAccepted { receipt } => DeliveryState::Imported {
            receipt: receipt.clone(),
        },
        FederationPeerFinality::ImportRejected { code } => DeliveryState::Rejected {
            stage: FederationDeliveryStage::Import,
            code: code.clone(),
        },
        FederationPeerFinality::InvocationAccepted { receipt, task } => DeliveryState::Accepted {
            receipt: receipt.clone(),
            task: task.clone(),
        },
        FederationPeerFinality::InvocationRejected { code, .. } => DeliveryState::Rejected {
            stage: FederationDeliveryStage::Invoke,
            code: code.clone(),
        },
    };
}

fn same_peer_finality(
    left: &FederationPeerFinality,
    right: &FederationPeerFinality,
) -> Result<bool, FederationTransportError> {
    Ok(
        serde_json::to_value(left).map_err(|_| FederationTransportError::Invalid)?
            == serde_json::to_value(right).map_err(|_| FederationTransportError::Invalid)?,
    )
}

fn valid_finality_acceptance(acceptance: &FederationFinalityAcceptance) -> bool {
    acceptance.accepted_at_ms >= 0
        && valid_text(acceptance.operator.id(), 255)
        && valid_text(&acceptance.authority.incarnation, 120)
}

fn valid_reconciliations(record: &DeliveryRecord) -> bool {
    if record.reconciliations.len() > 2 {
        return false;
    }
    if record.reconciliations.len() == 2
        && (!matches!(
            &record.reconciliations[0].finality,
            FederationPeerFinality::ImportAccepted { .. }
        ) || record.reconciliations[1].correlation.stage != FederationDeliveryStage::Invoke)
    {
        return false;
    }
    let mut previous = None;
    for reconciliation in &record.reconciliations {
        let stage = reconciliation.correlation.stage;
        if previous.is_some_and(|previous| {
            previous == stage
                || (previous == FederationDeliveryStage::Invoke
                    && stage == FederationDeliveryStage::Import)
        }) || !valid_text(&reconciliation.source_id, 255)
            || !valid_text(&reconciliation.qualification_ref, 255)
            || !valid_text(&reconciliation.verifier_revision, 120)
            || reconciliation.proof.is_empty()
            || reconciliation.proof.len() > MAX_FEDERATION_FINALITY_PROOF_BYTES
            || !valid_digest(&reconciliation.proof_digest)
            || format!("{:x}", Sha256::digest(&reconciliation.proof)) != reconciliation.proof_digest
            || !valid_finality_acceptance(&reconciliation.acceptance)
            || reconciliation.acceptance.accepted_at_ms < record.created_at_ms
            || finality_correlation(record, stage).ok().as_ref()
                != Some(&reconciliation.correlation)
            || validate_peer_finality(
                record,
                &reconciliation.correlation,
                &reconciliation.finality,
            )
            .is_err()
        {
            return false;
        }
        previous = Some(stage);
    }
    reconciliation_state_consistent(record)
}

fn reconciliation_state_consistent(record: &DeliveryRecord) -> bool {
    let Some(latest) = record.reconciliations.last() else {
        return true;
    };
    match (&latest.finality, &record.state) {
        (
            FederationPeerFinality::ImportAccepted { .. },
            DeliveryState::Imported { .. }
            | DeliveryState::Invoking { .. }
            | DeliveryState::InvokeUncertain { .. }
            | DeliveryState::Accepted { .. }
            | DeliveryState::Rejected {
                stage: FederationDeliveryStage::Invoke,
                ..
            }
            | DeliveryState::Abandoned {
                stage: FederationDeliveryStage::Invoke,
                ..
            },
        ) => true,
        (
            FederationPeerFinality::ImportRejected { code: expected },
            DeliveryState::Rejected {
                stage: FederationDeliveryStage::Import,
                code,
            },
        )
        | (
            FederationPeerFinality::InvocationRejected { code: expected, .. },
            DeliveryState::Rejected {
                stage: FederationDeliveryStage::Invoke,
                code,
            },
        ) => code == expected,
        (
            FederationPeerFinality::InvocationAccepted {
                receipt: expected_receipt,
                task: expected_task,
            },
            DeliveryState::Accepted { receipt, task },
        ) => {
            receipt == expected_receipt
                && serde_json::to_value(task).ok() == serde_json::to_value(expected_task).ok()
        }
        _ => false,
    }
}

fn valid_evidence_record(record: &DeliveryRecord) -> bool {
    if record.schema != DELIVERY_SCHEMA
        || !valid_text(&record.adapter_revision, 120)
        || !valid_digest(&record.binding_digest)
        || !valid_endpoint(&record.endpoint)
        || !valid_text(&record.agent_id, 120)
        || record.created_at_ms < 0
        || record.reconciliation_resources.is_empty()
        || record.reconciliation_resources.len() > 16
        || record
            .reconciliation_resources
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != record.reconciliation_resources.len()
        || record.reconciliation_resources.iter().any(|resource| {
            resource.namespace() != record.parent.namespace()
                || resource.tenant() != record.parent.tenant()
        })
        || record.message.task_id.is_some()
        || record.message.validate().is_err()
        || record.envelope.payload.len() > MAX_SIGNED_PAYLOAD_BYTES
        || record.invocation.payload.len() > MAX_SIGNED_PAYLOAD_BYTES
        || !valid_signature(&record.envelope.signature)
        || !valid_signature(&record.invocation.signature)
        || delivery_id(&record.parent, &record.invocation).ok() != Some(record.delivery_id)
        || !valid_reconciliations(record)
    {
        return false;
    }
    let Ok(envelope) = record.envelope.envelope() else {
        return false;
    };
    let Ok(invocation) = record.invocation.invocation() else {
        return false;
    };
    if invocation.import.id != envelope.id
        || record.envelope.digest().ok().as_deref()
            != Some(invocation.import.envelope_digest.as_str())
        || envelope.agent_resource.id() != record.agent_id
        || record.parent.principal() != &envelope.source
        || same_message(&invocation.message, &record.message).ok() != Some(true)
    {
        return false;
    }
    match &record.state {
        DeliveryState::Registered | DeliveryState::ImportUncertain => true,
        DeliveryState::Importing {
            claim_expires_at_ms,
            ..
        } => *claim_expires_at_ms >= record.created_at_ms,
        DeliveryState::Imported { receipt } | DeliveryState::InvokeUncertain { receipt } => {
            receipt_matches(record, receipt)
        }
        DeliveryState::Invoking {
            receipt,
            claim_expires_at_ms,
            ..
        } => receipt_matches(record, receipt) && *claim_expires_at_ms >= record.created_at_ms,
        DeliveryState::Accepted { receipt, task } => {
            receipt_matches(record, receipt) && task_matches(record, task)
        }
        DeliveryState::Rejected { code, .. } => valid_code(code),
        DeliveryState::Abandoned {
            stage,
            receipt,
            abandonment,
        } => {
            valid_abandonment(abandonment)
                && abandonment.recorded_at_ms >= record.created_at_ms
                && match (stage, receipt) {
                    (FederationDeliveryStage::Import, None) => true,
                    (FederationDeliveryStage::Invoke, Some(receipt)) => {
                        receipt_matches(record, receipt)
                    }
                    _ => false,
                }
        }
    }
}

fn project_evidence(
    record: &DeliveryRecord,
    now_ms: i64,
) -> Result<FederationDeliveryEvidence, FederationTransportError> {
    let envelope = record
        .envelope
        .envelope()
        .map_err(|_| FederationTransportError::Conflict)?;
    let invocation = record
        .invocation
        .invocation()
        .map_err(|_| FederationTransportError::Conflict)?;
    let status = match &record.state {
        DeliveryState::Registered => FederationDeliveryEvidenceStatus::Registered,
        DeliveryState::Importing {
            claim_expires_at_ms,
            ..
        } => FederationDeliveryEvidenceStatus::Importing {
            claim_expires_at_ms: *claim_expires_at_ms,
        },
        DeliveryState::ImportUncertain => FederationDeliveryEvidenceStatus::ImportUncertain,
        DeliveryState::Imported { .. } => FederationDeliveryEvidenceStatus::Imported,
        DeliveryState::Invoking {
            claim_expires_at_ms,
            ..
        } => FederationDeliveryEvidenceStatus::Invoking {
            claim_expires_at_ms: *claim_expires_at_ms,
        },
        DeliveryState::InvokeUncertain { .. } => FederationDeliveryEvidenceStatus::InvokeUncertain,
        DeliveryState::Accepted { .. } => FederationDeliveryEvidenceStatus::Accepted,
        DeliveryState::Rejected { stage, code } => FederationDeliveryEvidenceStatus::Rejected {
            stage: *stage,
            code: code.clone(),
        },
        DeliveryState::Abandoned {
            stage, abandonment, ..
        } => FederationDeliveryEvidenceStatus::Abandoned {
            stage: *stage,
            abandonment: abandonment.clone(),
        },
    };
    Ok(FederationDeliveryEvidence {
        delivery_id: record.delivery_id,
        parent_execution_id: record.parent.execution_id(),
        source: envelope.source,
        reconciliation_resources: record.reconciliation_resources.clone(),
        agent_id: record.agent_id.clone(),
        message_id: invocation.message.message_id,
        trust_id: envelope.trust_id,
        trust_revision: envelope.trust_revision,
        key_id: envelope.key_id,
        issuer_epoch: envelope.issuer_epoch,
        created_at_ms: record.created_at_ms,
        expires_at_ms: invocation.expires_at_ms,
        expired: now_ms >= invocation.expires_at_ms,
        exact_replay_idempotent: record.exact_replay_idempotent,
        status,
        reconciliations: record
            .reconciliations
            .iter()
            .map(|entry| FederationDeliveryFinalityEvidence {
                stage: entry.correlation.stage,
                accepted: entry.finality.accepted(),
                source_id: entry.source_id.clone(),
                qualification_ref: entry.qualification_ref.clone(),
                verifier_revision: entry.verifier_revision.clone(),
                proof_digest: entry.proof_digest.clone(),
                acceptance: entry.acceptance.clone(),
            })
            .collect(),
    })
}

fn delivery_id(
    parent: &ExecutionContextReference,
    invocation: &SignedFederationInvocation,
) -> Result<Uuid, FederationTransportError> {
    let digest = invocation
        .digest()
        .map_err(|_| FederationTransportError::Invalid)?;
    Ok(Uuid::new_v5(
        &parent.execution_id(),
        format!("acteon.federation-delivery.v1\0{digest}").as_bytes(),
    ))
}

fn receipt_matches(record: &DeliveryRecord, receipt: &FederationImportReceipt) -> bool {
    let Ok(envelope) = record.envelope.envelope() else {
        return false;
    };
    receipt.namespace == envelope.agent_resource.namespace()
        && receipt.tenant == envelope.agent_resource.tenant()
        && receipt.import_id == envelope.id
        && record.envelope.digest().ok().as_deref() == Some(receipt.envelope_digest.as_str())
        && receipt.trust_id == envelope.trust_id
        && receipt.trust_revision == envelope.trust_revision
        && receipt.issuer_domain == envelope.issuer_domain
        && receipt.source == envelope.source
        && receipt.target == envelope.target
        && receipt.agent_resource == envelope.agent_resource
        && receipt.binding_digest == envelope.binding_digest
        && receipt.skill == envelope.skill
        && receipt.expires_at_ms == envelope.expires_at_ms
        && receipt.accepted_at_ms >= 0
        && receipt.accepted_at_ms < receipt.expires_at_ms
        && !receipt.root_id.is_empty()
        && !receipt.execution_started
}

fn task_matches(record: &DeliveryRecord, task: &Task) -> bool {
    let Ok(envelope) = record.envelope.envelope() else {
        return false;
    };
    if task.validate().is_err()
        || task.namespace != envelope.agent_resource.namespace()
        || task.tenant != envelope.agent_resource.tenant()
        || task.history.is_empty()
    {
        return false;
    }
    let mut initial = task.history[0].clone();
    initial.task_id = None;
    same_message(&initial, &record.message).ok() == Some(true)
}

fn same_message(left: &TaskMessage, right: &TaskMessage) -> Result<bool, FederationTransportError> {
    Ok(
        serde_json::to_value(left).map_err(|_| FederationTransportError::Invalid)?
            == serde_json::to_value(right).map_err(|_| FederationTransportError::Invalid)?,
    )
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}

fn valid_code(value: &str) -> bool {
    valid_text(value, MAX_CODE_BYTES)
}

fn valid_abandonment(value: &FederationDeliveryAbandonment) -> bool {
    !value.disposition_id.is_nil()
        && valid_text(value.operator.id(), 255)
        && valid_text(&value.authority.incarnation, 120)
        && valid_text(&value.reason, MAX_CODE_BYTES)
        && value.recorded_at_ms >= 0
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_signature(value: &str) -> bool {
    value.len() == 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_endpoint(value: &str) -> bool {
    if value.len() > MAX_ENDPOINT_BYTES || value.trim() != value {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && !url.cannot_be_a_base()
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}
