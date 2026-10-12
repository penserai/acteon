//! Independently qualified finality for outbound federation delivery.
//!
//! A proof is evidence about a request that may already have crossed the
//! network boundary. It is never authority to send or replay that request.

use std::collections::BTreeMap;

use acteon_core::{FederationImportReceipt, PrincipalIdentity, Task};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::{
    federation_transport::{FederationDeliveryStage, FederationTransportError},
    plan::canonical_bytes,
};

pub const MAX_FEDERATION_FINALITY_PROOF_BYTES: usize = 64 * 1024;

/// Immutable correlation derived from the retained signed delivery. It is not
/// a credential, permit, lease, or replay capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFinalityCorrelation {
    pub delivery_id: Uuid,
    pub stage: FederationDeliveryStage,
    pub parent_execution_id: Uuid,
    pub source: PrincipalIdentity,
    pub adapter_revision: String,
    pub binding_digest: String,
    pub envelope_digest: String,
    pub invocation_digest: String,
    pub import_id: String,
    pub invocation_nonce: String,
    pub agent_id: String,
    pub message_id: String,
}

/// A qualified source may establish only the result of the exact ambiguous
/// stage. Import acceptance is non-terminal and merely permits the source to
/// recheck authority before attempting invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationPeerFinality {
    ImportAccepted {
        receipt: Box<FederationImportReceipt>,
    },
    ImportRejected {
        code: String,
    },
    InvocationAccepted {
        receipt: Box<FederationImportReceipt>,
        task: Box<Task>,
    },
    InvocationRejected {
        receipt: Box<FederationImportReceipt>,
        code: String,
    },
}

impl FederationPeerFinality {
    #[must_use]
    pub fn stage(&self) -> FederationDeliveryStage {
        match self {
            Self::ImportAccepted { .. } | Self::ImportRejected { .. } => {
                FederationDeliveryStage::Import
            }
            Self::InvocationAccepted { .. } | Self::InvocationRejected { .. } => {
                FederationDeliveryStage::Invoke
            }
        }
    }

    #[must_use]
    pub fn accepted(&self) -> bool {
        matches!(
            self,
            Self::ImportAccepted { .. } | Self::InvocationAccepted { .. }
        )
    }
}

/// Installed by the trusted host for one exact outbound binding. Verifying a
/// signature alone is insufficient: an implementation must qualify how the
/// peer maps this correlation to irrevocable stage finality.
pub trait FederationFinalityVerifier: Send + Sync {
    fn source_id(&self) -> &str;
    fn qualification_ref(&self) -> &str;
    fn revision(&self) -> &str;

    fn verify(
        &self,
        correlation: &FederationFinalityCorrelation,
        proof: &[u8],
    ) -> Result<FederationPeerFinality, FederationTransportError>;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedFederationFinalityReceipt {
    schema: u32,
    verifier_revision: String,
    key_id: String,
    correlation: FederationFinalityCorrelation,
    finality: FederationPeerFinality,
    signature: String,
}

fn signature_bytes(
    receipt: &SignedFederationFinalityReceipt,
) -> Result<Vec<u8>, FederationTransportError> {
    canonical_bytes(&serde_json::json!({
        "domain": "acteon.federation.finality.v1",
        "schema": receipt.schema,
        "verifier_revision": receipt.verifier_revision,
        "key_id": receipt.key_id,
        "correlation": receipt.correlation,
        "finality": receipt.finality,
    }))
    .map_err(|_| FederationTransportError::Invalid)
}

/// Local verifier for a separately qualified finality source. Keys must be
/// dedicated to finality and must not be peer credentials or signing keys.
pub struct HmacFederationFinalityVerifier {
    source_id: String,
    qualification_ref: String,
    revision: String,
    keys: BTreeMap<String, Vec<u8>>,
}

impl HmacFederationFinalityVerifier {
    pub fn new_trusted(
        source_id: &str,
        qualification_ref: &str,
        revision: &str,
        keys: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, FederationTransportError> {
        if !valid_text(source_id)
            || !valid_text(qualification_ref)
            || !valid_text(revision)
            || keys.is_empty()
            || keys.len() > 16
            || keys
                .iter()
                .any(|(id, key)| !valid_text(id) || !(32..=1024).contains(&key.len()))
        {
            return Err(FederationTransportError::Invalid);
        }
        Ok(Self {
            source_id: source_id.into(),
            qualification_ref: qualification_ref.into(),
            revision: revision.into(),
            keys,
        })
    }
}

impl FederationFinalityVerifier for HmacFederationFinalityVerifier {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn qualification_ref(&self) -> &str {
        &self.qualification_ref
    }

    fn revision(&self) -> &str {
        &self.revision
    }

    fn verify(
        &self,
        correlation: &FederationFinalityCorrelation,
        proof: &[u8],
    ) -> Result<FederationPeerFinality, FederationTransportError> {
        if proof.is_empty() || proof.len() > MAX_FEDERATION_FINALITY_PROOF_BYTES {
            return Err(FederationTransportError::Invalid);
        }
        let receipt: SignedFederationFinalityReceipt =
            serde_json::from_slice(proof).map_err(|_| FederationTransportError::Invalid)?;
        if receipt.schema != 1
            || receipt.verifier_revision != self.revision
            || receipt.correlation != *correlation
            || receipt.finality.stage() != correlation.stage
        {
            return Err(FederationTransportError::Conflict);
        }
        let key = self
            .keys
            .get(&receipt.key_id)
            .ok_or(FederationTransportError::Refused)?;
        let signature = STANDARD
            .decode(&receipt.signature)
            .map_err(|_| FederationTransportError::Invalid)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(key).map_err(|_| FederationTransportError::Invalid)?;
        mac.update(&signature_bytes(&receipt)?);
        mac.verify_slice(&signature)
            .map_err(|_| FederationTransportError::Refused)?;
        Ok(receipt.finality)
    }
}

/// Helper for an independently qualified finality issuer. Calling it neither
/// installs trust nor changes a delivery.
pub fn sign_federation_finality_receipt(
    correlation: FederationFinalityCorrelation,
    finality: FederationPeerFinality,
    verifier_revision: &str,
    key_id: &str,
    key: &[u8],
) -> Result<Vec<u8>, FederationTransportError> {
    if finality.stage() != correlation.stage
        || !valid_text(verifier_revision)
        || !valid_text(key_id)
        || !(32..=1024).contains(&key.len())
    {
        return Err(FederationTransportError::Invalid);
    }
    let mut receipt = SignedFederationFinalityReceipt {
        schema: 1,
        verifier_revision: verifier_revision.into(),
        key_id: key_id.into(),
        correlation,
        finality,
        signature: String::new(),
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| FederationTransportError::Invalid)?;
    mac.update(&signature_bytes(&receipt)?);
    receipt.signature = STANDARD.encode(mac.finalize().into_bytes());
    let proof = serde_json::to_vec(&receipt).map_err(|_| FederationTransportError::Invalid)?;
    if proof.len() > MAX_FEDERATION_FINALITY_PROOF_BYTES {
        return Err(FederationTransportError::Invalid);
    }
    Ok(proof)
}

fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}
