//! Explicit cross-domain trust and audience-bound delegation imports.
//!
//! The foreign signature authenticates an assertion. Local policy decides how
//! much of that assertion may be imported, and the coordinator serializes trust
//! changes and imports with local effect authority. An imported record is not a
//! bearer token: a trusted runtime must recheck it immediately before deriving
//! local execution context or registering an effect.

use std::collections::BTreeSet;

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_time::Clock;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::context::AcceptedEffect;
use crate::permit::{matches_effect, valid_effects};
use crate::{
    AttemptRequest, AuthorityChange, AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE,
    CONTROL_RECORD_RESERVE, ChangeRecord, CoordinationError, CoordinatorSnapshot, RETRIES,
    RootBudget, RootBudgetLimits, RootReservation, ScopePurpose, StartRegistration, valid_text,
};

const ENVELOPE_SCHEMA: u32 = 1;
const MAX_APPROVED_SHAPES: usize = 128;
const MAX_CLOCK_BOUND_MS: i64 = 24 * 60 * 60 * 1000;

/// Exact locally reviewed import shape. Effects may be attenuated, while actor,
/// recipient, service binding and ingress operation must match exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationGrantCeiling {
    pub source: PrincipalIdentity,
    pub target: PrincipalIdentity,
    pub agent_resource: ResourceRef,
    pub binding_digest: String,
    pub skill: String,
    pub ingress_effect: AcceptedEffect,
    pub effects: Vec<AcceptedEffect>,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
}

/// A local operator's explicit trust in one foreign signing key and bounded
/// import policy. Revocation is terminal for this trust ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTrust {
    pub id: String,
    pub revision: u64,
    pub foreign_domain: String,
    pub local_audience: String,
    pub key_id: String,
    /// Lowercase hex Ed25519 public key. The private key never enters authority state.
    pub verifying_key: String,
    /// Reject envelopes issued from an older foreign revocation epoch.
    pub minimum_issuer_epoch: u64,
    /// Maximum age of the issuer's authority observation at local acceptance.
    pub max_revocation_staleness_ms: i64,
    /// Permitted positive clock skew when checking `issued_at_ms`.
    pub max_clock_skew_ms: i64,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
    pub approved: Vec<FederationGrantCeiling>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTrustRecord {
    pub trust: FederationTrust,
    pub revoked: bool,
}

/// Signed, schema-closed foreign assertion. Authority is still bounded by the
/// current local trust policy at import and at every later use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationEnvelope {
    pub schema_version: u32,
    pub id: String,
    pub trust_id: String,
    pub trust_revision: u64,
    pub issuer_domain: String,
    pub audience: String,
    pub key_id: String,
    pub issuer_epoch: u64,
    pub authority_observed_at_ms: i64,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub source: PrincipalIdentity,
    pub target: PrincipalIdentity,
    pub agent_resource: ResourceRef,
    pub binding_digest: String,
    pub skill: String,
    pub ingress_effect: AcceptedEffect,
    pub effects: Vec<AcceptedEffect>,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
}

impl FederationEnvelope {
    /// Construct the only supported envelope schema. Validation also occurs at
    /// signing and import so deserialized data never bypasses bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        trust_id: impl Into<String>,
        trust_revision: u64,
        issuer_domain: impl Into<String>,
        audience: impl Into<String>,
        key_id: impl Into<String>,
        issuer_epoch: u64,
        authority_observed_at_ms: i64,
        issued_at_ms: i64,
        expires_at_ms: i64,
        source: PrincipalIdentity,
        target: PrincipalIdentity,
        agent_resource: ResourceRef,
        binding_digest: impl Into<String>,
        skill: impl Into<String>,
        ingress_effect: AcceptedEffect,
        effects: Vec<AcceptedEffect>,
        limits: RootBudgetLimits,
        max_depth: usize,
    ) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA,
            id: id.into(),
            trust_id: trust_id.into(),
            trust_revision,
            issuer_domain: issuer_domain.into(),
            audience: audience.into(),
            key_id: key_id.into(),
            issuer_epoch,
            authority_observed_at_ms,
            issued_at_ms,
            expires_at_ms,
            source,
            target,
            agent_resource,
            binding_digest: binding_digest.into(),
            skill: skill.into(),
            ingress_effect,
            effects,
            limits,
            max_depth,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedFederationEnvelope {
    /// Exact UTF-8 JSON bytes signed by the foreign domain. Verification never
    /// reparses and reserializes before checking the signature.
    pub payload: String,
    /// Lowercase hex Ed25519 signature over `payload.as_bytes()`.
    pub signature: String,
}

impl SignedFederationEnvelope {
    pub fn envelope(&self) -> Result<FederationEnvelope, CoordinationError> {
        serde_json::from_str(&self.payload)
            .map_err(|_| CoordinationError::Invalid("federation payload".into()))
    }
}

/// Export-only signer. It deliberately has no serializer and exposes no key.
pub struct FederationEnvelopeSigner {
    key: SigningKey,
}

impl FederationEnvelopeSigner {
    #[must_use]
    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(&secret),
        }
    }

    #[must_use]
    pub fn verifying_key_hex(&self) -> String {
        encode_hex(self.key.verifying_key().as_bytes())
    }

    pub fn sign(
        &self,
        envelope: &FederationEnvelope,
    ) -> Result<SignedFederationEnvelope, CoordinationError> {
        validate_envelope_shape(envelope)?;
        let payload = serde_json::to_string(&envelope)
            .map_err(|_| CoordinationError::Invalid("federation payload".into()))?;
        Ok(SignedFederationEnvelope {
            signature: encode_hex(&self.key.sign(payload.as_bytes()).to_bytes()),
            payload,
        })
    }
}

/// Durable imported assertion. It retains the exact signed-envelope digest and
/// the local authority generation that accepted it, without storing a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationImportRecord {
    pub id: String,
    pub envelope_digest: String,
    pub trust_id: String,
    pub trust_revision: u64,
    pub issuer_domain: String,
    pub issuer_epoch: u64,
    pub source: PrincipalIdentity,
    pub target: PrincipalIdentity,
    pub agent_resource: ResourceRef,
    pub binding_digest: String,
    pub skill: String,
    pub ingress_effect: AcceptedEffect,
    pub effects: Vec<AcceptedEffect>,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
    /// Local funded root created atomically with the import.
    pub root_id: String,
    pub expires_at_ms: i64,
    pub accepted_at_ms: i64,
    pub authority: AuthorityStamp,
    /// Exact foreign evidence retained for restart verification and corruption detection.
    pub signed_envelope: SignedFederationEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationImportReference {
    pub id: String,
    pub envelope_digest: String,
}

/// Trusted host ceiling for trust-root publication. Public requests cannot
/// construct this value through deserialization.
pub struct FederationTrustIssuanceCeiling {
    pub issuer: PrincipalIdentity,
    pub approved: Vec<FederationTrust>,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
}

pub struct EvaluatedFederationTrustPublication<'a> {
    pub change_id: &'a str,
    pub trust: FederationTrust,
    pub expected_revision: u64,
    pub ceiling: &'a FederationTrustIssuanceCeiling,
    pub evaluated_authority: &'a AuthorityStamp,
    pub reason: &'a str,
    pub clock: &'a dyn Clock,
}

pub struct FederatedDelegationImport<'a> {
    pub signed: &'a SignedFederationEnvelope,
    pub expected_authority: &'a AuthorityStamp,
    pub clock: &'a dyn Clock,
}

/// One locally executed effect under a current federated import. The effect,
/// reservation and trust are checked in the same coordinator CAS as the start.
pub struct FederatedAttempt<'a> {
    pub id: &'a str,
    pub import: &'a FederationImportReference,
    pub effect: &'a AcceptedEffect,
    pub request_digest: &'a str,
    pub units: u64,
    pub expected_authority: &'a AuthorityStamp,
    pub clock: &'a dyn Clock,
}

pub(crate) struct FederationAttemptCheck<'a> {
    pub import: &'a FederationImportReference,
    pub effect: &'a AcceptedEffect,
    pub clock: &'a dyn Clock,
}

fn digest_valid(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_limits(limits: &RootBudgetLimits) -> bool {
    limits.max_units > 0 && limits.max_concurrent > 0 && limits.deadline_ms > 0
}

fn bounded_limits(value: &RootBudgetLimits, ceiling: &RootBudgetLimits) -> bool {
    valid_limits(value)
        && value.max_units <= ceiling.max_units
        && value.max_concurrent <= ceiling.max_concurrent
        && value.deadline_ms <= ceiling.deadline_ms
}

fn valid_ceiling_shape(shape: &FederationGrantCeiling) -> bool {
    valid_text(shape.source.id())
        && valid_text(shape.target.id())
        && digest_valid(&shape.binding_digest, 32)
        && valid_text(&shape.skill)
        && shape.skill.len() <= 120
        && shape.target.kind() == PrincipalKind::Agent
        && shape.agent_resource.kind() == ResourceKind::Agent
        && shape.ingress_effect.operation == "agent.invoke"
        && valid_effects(std::slice::from_ref(&shape.ingress_effect))
        && shape
            .ingress_effect
            .resources
            .contains(&shape.agent_resource)
        && valid_effects(&shape.effects)
        && shape.effects.iter().all(|effect| {
            effect
                .resources
                .iter()
                .all(|resource| shape.ingress_effect.resources.contains(resource))
        })
        && valid_limits(&shape.limits)
        && shape.max_depth > 0
}

pub(crate) fn valid_trust(trust: &FederationTrust) -> bool {
    valid_text(&trust.id)
        && trust.revision > 0
        && valid_text(&trust.foreign_domain)
        && valid_text(&trust.local_audience)
        && trust.foreign_domain != trust.local_audience
        && valid_text(&trust.key_id)
        && digest_valid(&trust.verifying_key, 32)
        && decode_hex::<32>(&trust.verifying_key)
            .ok()
            .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
            .is_some()
        && trust.minimum_issuer_epoch > 0
        && (1..=MAX_CLOCK_BOUND_MS).contains(&trust.max_revocation_staleness_ms)
        && (0..=MAX_CLOCK_BOUND_MS).contains(&trust.max_clock_skew_ms)
        && trust.valid_from_ms >= 0
        && trust.deadline_ms > trust.valid_from_ms
        && !trust.approved.is_empty()
        && trust.approved.len() <= MAX_APPROVED_SHAPES
        && trust.approved.iter().all(valid_ceiling_shape)
}

fn validate_envelope_shape(envelope: &FederationEnvelope) -> Result<(), CoordinationError> {
    if envelope.schema_version != ENVELOPE_SCHEMA
        || ![
            envelope.id.as_str(),
            envelope.trust_id.as_str(),
            envelope.issuer_domain.as_str(),
            envelope.audience.as_str(),
            envelope.key_id.as_str(),
            envelope.binding_digest.as_str(),
            envelope.skill.as_str(),
        ]
        .into_iter()
        .all(valid_text)
        || !digest_valid(&envelope.binding_digest, 32)
        || envelope.trust_revision == 0
        || envelope.issuer_epoch == 0
        || envelope.authority_observed_at_ms < 0
        || envelope.issued_at_ms < envelope.authority_observed_at_ms
        || envelope.expires_at_ms <= envelope.issued_at_ms
        || envelope.limits.deadline_ms > envelope.expires_at_ms
        || !valid_effects(std::slice::from_ref(&envelope.ingress_effect))
        || !valid_effects(&envelope.effects)
        || !valid_limits(&envelope.limits)
        || envelope.max_depth == 0
    {
        return Err(CoordinationError::Invalid("federation envelope".into()));
    }
    Ok(())
}

fn envelope_digest(signed: &SignedFederationEnvelope) -> Result<String, CoordinationError> {
    let raw = serde_json::to_vec(signed)
        .map_err(|_| CoordinationError::Invalid("federation envelope".into()))?;
    Ok(format!("{:x}", Sha256::digest(raw)))
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], CoordinationError> {
    if !digest_valid(value, N) {
        return Err(CoordinationError::Invalid(
            "federation key or signature".into(),
        ));
    }
    let mut out = [0_u8; N];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| CoordinationError::Invalid("federation key or signature".into()))?;
    }
    Ok(out)
}

fn encode_hex(value: &[u8]) -> String {
    use std::fmt::Write as _;
    value
        .iter()
        .fold(String::with_capacity(value.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn verify_signature(
    trust: &FederationTrust,
    signed: &SignedFederationEnvelope,
) -> Result<(), CoordinationError> {
    let key = VerifyingKey::from_bytes(&decode_hex::<32>(&trust.verifying_key)?)
        .map_err(|_| CoordinationError::Invalid("federation public key".into()))?;
    let signature = Signature::from_bytes(&decode_hex::<64>(&signed.signature)?);
    key.verify_strict(signed.payload.as_bytes(), &signature)
        .map_err(|_| CoordinationError::Restricted)
}

fn approved_by(trust: &FederationTrust, envelope: &FederationEnvelope) -> bool {
    trust.approved.iter().any(|ceiling| {
        ceiling.source == envelope.source
            && ceiling.target == envelope.target
            && ceiling.agent_resource == envelope.agent_resource
            && ceiling.binding_digest == envelope.binding_digest
            && ceiling.skill == envelope.skill
            && matches_effect(&ceiling.ingress_effect, &envelope.ingress_effect)
            && envelope.effects.iter().all(|effect| {
                ceiling
                    .effects
                    .iter()
                    .any(|allowed| matches_effect(allowed, effect))
            })
            && bounded_limits(&envelope.limits, &ceiling.limits)
            && envelope.max_depth <= ceiling.max_depth
    })
}

fn valid_historical_acceptance(
    trust: &FederationTrust,
    envelope: &FederationEnvelope,
    accepted_at_ms: i64,
    max_active: usize,
) -> bool {
    validate_envelope_shape(envelope).is_ok()
        && envelope.trust_id == trust.id
        && envelope.trust_revision == trust.revision
        && envelope.issuer_domain == trust.foreign_domain
        && envelope.audience == trust.local_audience
        && envelope.key_id == trust.key_id
        && envelope.issuer_epoch >= trust.minimum_issuer_epoch
        && accepted_at_ms >= trust.valid_from_ms
        && accepted_at_ms < trust.deadline_ms
        && envelope.issued_at_ms <= accepted_at_ms.saturating_add(trust.max_clock_skew_ms)
        && accepted_at_ms >= envelope.authority_observed_at_ms
        && accepted_at_ms.saturating_sub(envelope.authority_observed_at_ms)
            <= trust.max_revocation_staleness_ms
        && envelope.expires_at_ms
            <= envelope
                .authority_observed_at_ms
                .saturating_add(trust.max_revocation_staleness_ms)
        && accepted_at_ms < envelope.expires_at_ms
        && envelope.limits.max_concurrent <= u64::try_from(max_active).unwrap_or(u64::MAX)
        && approved_by(trust, envelope)
}

fn authority_allowed_at_acceptance(
    events: &[&ChangeRecord],
    import: &FederationImportRecord,
) -> bool {
    let mut active_trust_revision = None;
    let mut closed_resources = BTreeSet::new();
    let mut target_revoked = false;
    for event in events
        .iter()
        .filter(|event| event.generation <= import.authority.generation)
    {
        match &event.change {
            AuthorityChange::PublishFederationTrust { trust } if trust.id == import.trust_id => {
                active_trust_revision = Some(trust.revision);
            }
            AuthorityChange::RevokeFederationTrust { trust_id, .. }
                if trust_id == &import.trust_id =>
            {
                active_trust_revision = None;
            }
            AuthorityChange::CloseResource { resource } => {
                closed_resources.insert(resource.clone());
            }
            AuthorityChange::ReopenResource { resource } => {
                closed_resources.remove(resource);
            }
            AuthorityChange::RevokeSubject { subject } if subject == import.target.id() => {
                target_revoked = true;
            }
            _ => {}
        }
    }
    active_trust_revision == Some(import.trust_revision)
        && !target_revoked
        && std::iter::once(&import.ingress_effect)
            .chain(&import.effects)
            .flat_map(|effect| &effect.resources)
            .all(|resource| !closed_resources.contains(resource))
}

fn evaluate(
    coordinator: &AuthorityCoordinator,
    state: &CoordinatorSnapshot,
    signed: &SignedFederationEnvelope,
    now_ms: i64,
) -> Result<FederationImportRecord, CoordinationError> {
    let envelope = signed.envelope()?;
    validate_envelope_shape(&envelope)?;
    coordinator.validate_resource_scope(&envelope.agent_resource)?;
    for effect in std::iter::once(&envelope.ingress_effect).chain(&envelope.effects) {
        for resource in &effect.resources {
            coordinator.validate_resource_scope(resource)?;
        }
    }
    let record = state
        .federation_trusts
        .get(&envelope.trust_id)
        .ok_or(CoordinationError::Restricted)?;
    let trust = &record.trust;
    if state.purpose != ScopePurpose::Execution
        || record.revoked
        || envelope.trust_revision != trust.revision
        || envelope.issuer_domain != trust.foreign_domain
        || envelope.audience != trust.local_audience
        || envelope.key_id != trust.key_id
        || envelope.issuer_epoch < trust.minimum_issuer_epoch
        || now_ms < trust.valid_from_ms
        || now_ms >= trust.deadline_ms
        || envelope.issued_at_ms > now_ms.saturating_add(trust.max_clock_skew_ms)
        || now_ms < envelope.authority_observed_at_ms
        || now_ms.saturating_sub(envelope.authority_observed_at_ms)
            > trust.max_revocation_staleness_ms
        || envelope.expires_at_ms
            > envelope
                .authority_observed_at_ms
                .saturating_add(trust.max_revocation_staleness_ms)
        || now_ms >= envelope.expires_at_ms
        || envelope.limits.max_concurrent
            > u64::try_from(state.limits.max_active).unwrap_or(u64::MAX)
        || state.revoked_subjects.contains(envelope.target.id())
        || std::iter::once(&envelope.ingress_effect)
            .chain(&envelope.effects)
            .flat_map(|effect| &effect.resources)
            .any(|resource| state.closed_resources.contains(resource))
        || !approved_by(trust, &envelope)
    {
        return Err(CoordinationError::Restricted);
    }
    verify_signature(trust, signed)?;
    Ok(FederationImportRecord {
        id: envelope.id.clone(),
        envelope_digest: envelope_digest(signed)?,
        trust_id: trust.id.clone(),
        trust_revision: trust.revision,
        issuer_domain: envelope.issuer_domain.clone(),
        issuer_epoch: envelope.issuer_epoch,
        source: envelope.source.clone(),
        target: envelope.target.clone(),
        agent_resource: envelope.agent_resource.clone(),
        binding_digest: envelope.binding_digest.clone(),
        skill: envelope.skill.clone(),
        ingress_effect: envelope.ingress_effect.clone(),
        effects: envelope.effects.clone(),
        limits: envelope.limits.clone(),
        max_depth: envelope.max_depth,
        root_id: format!("federation:{}", envelope_digest(signed)?),
        expires_at_ms: envelope.expires_at_ms,
        accepted_at_ms: now_ms,
        authority: state.stamp(),
        signed_envelope: signed.clone(),
    })
}

impl AuthorityCoordinator {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn valid_federation_history(&self, state: &CoordinatorSnapshot) -> bool {
        let mut reconstructed = std::collections::BTreeMap::new();
        let mut trust_history = std::collections::BTreeMap::new();
        let mut events: Vec<_> = state.changes.values().collect();
        events.sort_by_key(|event| event.generation);
        for event in &events {
            match &event.change {
                AuthorityChange::PublishFederationTrust { trust } => {
                    let previous = reconstructed.get(&trust.id);
                    if !valid_trust(trust)
                        || previous.is_some_and(|record: &FederationTrustRecord| record.revoked)
                        || previous.map_or(Some(1), |record| record.trust.revision.checked_add(1))
                            != Some(trust.revision)
                        || previous.is_some_and(|record| {
                            record.trust.foreign_domain != trust.foreign_domain
                                || record.trust.local_audience != trust.local_audience
                        })
                    {
                        return false;
                    }
                    reconstructed.insert(
                        trust.id.clone(),
                        FederationTrustRecord {
                            trust: trust.clone(),
                            revoked: false,
                        },
                    );
                    trust_history.insert((trust.id.clone(), trust.revision), trust.clone());
                }
                AuthorityChange::RevokeFederationTrust {
                    trust_id,
                    expected_revision,
                } => {
                    let Some(record) = reconstructed.get_mut(trust_id) else {
                        return false;
                    };
                    if record.revoked || record.trust.revision != *expected_revision {
                        return false;
                    }
                    record.revoked = true;
                }
                _ => {}
            }
        }
        reconstructed == state.federation_trusts
            && state.federation_trusts.iter().all(|(id, record)| {
                id == &record.trust.id
                    && record.trust.approved.iter().all(|shape| {
                        self.validate_resource_scope(&shape.agent_resource).is_ok()
                            && std::iter::once(&shape.ingress_effect)
                                .chain(&shape.effects)
                                .flat_map(|effect| &effect.resources)
                                .all(|resource| self.validate_resource_scope(resource).is_ok())
                    })
            })
            && state.federation_imports.iter().all(|(id, import)| {
                let Ok(envelope) = import.signed_envelope.envelope() else {
                    return false;
                };
                let historical_trust =
                    trust_history.get(&(import.trust_id.clone(), import.trust_revision));
                id == &import.id
                    && digest_valid(&import.envelope_digest, 32)
                    && envelope_digest(&import.signed_envelope).ok().as_ref()
                        == Some(&import.envelope_digest)
                    && valid_text(&import.issuer_domain)
                    && valid_text(&import.trust_id)
                    && import.trust_revision > 0
                    && import.issuer_epoch > 0
                    && import.expires_at_ms > import.accepted_at_ms
                    && import.authority.incarnation == state.incarnation
                    && import.authority.generation <= state.generation
                    && authority_allowed_at_acceptance(&events, import)
                    && self.validate_resource_scope(&import.agent_resource).is_ok()
                    && valid_effects(std::slice::from_ref(&import.ingress_effect))
                    && valid_effects(&import.effects)
                    && valid_limits(&import.limits)
                    && import.max_depth > 0
                    && valid_text(&import.root_id)
                    && state.roots.get(&import.root_id).is_some_and(|root| {
                        root.owner_subject == import.target.id()
                            && root.limits == import.limits
                            && root.accepted_context.is_none()
                    })
                    && !state
                        .budget_parents
                        .values()
                        .any(|parent| parent == &import.root_id)
                    && historical_trust.is_some_and(|trust| {
                        verify_signature(trust, &import.signed_envelope).is_ok()
                            && valid_historical_acceptance(
                                trust,
                                &envelope,
                                import.accepted_at_ms,
                                state.limits.max_active,
                            )
                            && envelope.id == import.id
                            && envelope.issuer_domain == import.issuer_domain
                            && envelope.issuer_epoch == import.issuer_epoch
                            && envelope.source == import.source
                            && envelope.target == import.target
                            && envelope.agent_resource == import.agent_resource
                            && envelope.binding_digest == import.binding_digest
                            && envelope.skill == import.skill
                            && envelope.ingress_effect == import.ingress_effect
                            && envelope.effects == import.effects
                            && envelope.limits == import.limits
                            && envelope.max_depth == import.max_depth
                            && import.root_id == format!("federation:{}", import.envelope_digest)
                            && envelope.expires_at_ms == import.expires_at_ms
                    })
            })
    }

    pub async fn publish_federation_trust(
        &self,
        request: EvaluatedFederationTrustPublication<'_>,
    ) -> Result<ChangeRecord, CoordinationError> {
        let EvaluatedFederationTrustPublication {
            change_id,
            trust,
            expected_revision,
            ceiling,
            evaluated_authority,
            reason,
            clock,
        } = request;
        if !valid_trust(&trust)
            || !ceiling.approved.contains(&trust)
            || expected_revision.checked_add(1) != Some(trust.revision)
        {
            return Err(CoordinationError::Restricted);
        }
        self.change_federation_trust(
            change_id,
            AuthorityChange::PublishFederationTrust { trust },
            expected_revision,
            ceiling,
            evaluated_authority,
            reason,
            clock,
        )
        .await
    }

    /// Terminal local revocation. A new relationship requires a new trust ID;
    /// retained imports remain inspectable but cannot authorize future work.
    #[allow(clippy::too_many_arguments)]
    pub async fn revoke_federation_trust_evaluated(
        &self,
        change_id: &str,
        trust_id: &str,
        expected_revision: u64,
        ceiling: &FederationTrustIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        clock: &dyn Clock,
    ) -> Result<ChangeRecord, CoordinationError> {
        self.change_federation_trust(
            change_id,
            AuthorityChange::RevokeFederationTrust {
                trust_id: trust_id.into(),
                expected_revision,
            },
            expected_revision,
            ceiling,
            evaluated_authority,
            reason,
            clock,
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn change_federation_trust(
        &self,
        change_id: &str,
        change: AuthorityChange,
        expected_revision: u64,
        ceiling: &FederationTrustIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        clock: &dyn Clock,
    ) -> Result<ChangeRecord, CoordinationError> {
        if !valid_text(change_id)
            || !valid_text(reason)
            || ceiling.approved.is_empty()
            || ceiling.approved.len() > MAX_APPROVED_SHAPES
            || ceiling.approved.iter().any(|trust| !valid_trust(trust))
            || ceiling.valid_from_ms < 0
            || ceiling.deadline_ms <= ceiling.valid_from_ms
        {
            return Err(CoordinationError::Restricted);
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let now_ms = clock.now().timestamp_millis();
            if state.purpose != ScopePurpose::Execution || state.stamp() != *evaluated_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if now_ms < ceiling.valid_from_ms
                || now_ms >= ceiling.deadline_ms
                || state.revoked_subjects.contains(ceiling.issuer.id())
            {
                return Err(CoordinationError::Restricted);
            }
            let (trust_id, proposed) = match &change {
                AuthorityChange::PublishFederationTrust { trust } => {
                    (trust.id.as_str(), Some(trust))
                }
                AuthorityChange::RevokeFederationTrust { trust_id, .. } => {
                    (trust_id.as_str(), None)
                }
                _ => unreachable!("private federation trust change"),
            };
            let current = state.federation_trusts.get(trust_id);
            if let Some(existing) = state.changes.get(change_id) {
                if existing.change != change
                    || existing.actor != ceiling.issuer.id()
                    || existing.reason != reason
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(existing.clone());
            }
            if current.map_or(0, |record| record.trust.revision) != expected_revision
                || current.is_some_and(|record| record.revoked)
            {
                return Err(CoordinationError::Conflict);
            }
            if let Some(trust) = proposed {
                if !ceiling.approved.contains(trust)
                    || trust.deadline_ms <= now_ms
                    || current.is_some_and(|record| {
                        record.trust.foreign_domain != trust.foreign_domain
                            || record.trust.local_audience != trust.local_audience
                    })
                    || trust.approved.iter().any(|shape| {
                        self.validate_resource_scope(&shape.agent_resource).is_err()
                            || std::iter::once(&shape.ingress_effect)
                                .chain(&shape.effects)
                                .flat_map(|effect| &effect.resources)
                                .any(|resource| self.validate_resource_scope(resource).is_err())
                    })
                {
                    return Err(CoordinationError::Restricted);
                }
            } else if !current.is_some_and(|record| ceiling.approved.contains(&record.trust)) {
                return Err(CoordinationError::Restricted);
            }
            let reserve_records = if proposed.is_some() {
                CONTROL_RECORD_RESERVE
            } else {
                0
            };
            let reserve_bytes = if proposed.is_some() {
                CONTROL_BYTE_RESERVE
            } else {
                0
            };
            if state.record_count() >= state.limits.max_records - reserve_records {
                return Err(CoordinationError::Capacity);
            }
            match &change {
                AuthorityChange::PublishFederationTrust { trust } => {
                    state.federation_trusts.insert(
                        trust.id.clone(),
                        FederationTrustRecord {
                            trust: trust.clone(),
                            revoked: false,
                        },
                    );
                }
                AuthorityChange::RevokeFederationTrust {
                    trust_id,
                    expected_revision,
                } => Self::revoke_federation_trust(&mut state, trust_id, *expected_revision)?,
                _ => unreachable!("private federation trust change"),
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            let record = ChangeRecord {
                change: change.clone(),
                actor: ceiling.issuer.id().into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.changes.insert(change_id.into(), record.clone());
            if state.record_count() > state.limits.max_records - reserve_records
                || Self::encode(&state)?.len() > state.limits.max_bytes - reserve_bytes
            {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(record);
            }
        }
        Err(CoordinationError::Contention)
    }

    /// Verify and durably import one assertion. Replay observes the same record;
    /// a reused ID with different signed bytes conflicts.
    pub async fn import_federated_delegation(
        &self,
        request: FederatedDelegationImport<'_>,
    ) -> Result<FederationImportRecord, CoordinationError> {
        let digest = envelope_digest(request.signed)?;
        let envelope = request.signed.envelope()?;
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if state.stamp() != *request.expected_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if let Some(existing) = state.federation_imports.get(&envelope.id) {
                if existing.envelope_digest != digest {
                    return Err(CoordinationError::Conflict);
                }
                Self::check_federation_import_in_snapshot(
                    &state,
                    &FederationImportReference {
                        id: existing.id.clone(),
                        envelope_digest: existing.envelope_digest.clone(),
                    },
                    request.clock.now().timestamp_millis(),
                )?;
                return Ok(existing.clone());
            }
            if state.record_count() >= state.limits.max_records - CONTROL_RECORD_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            let mut imported = evaluate(
                self,
                &state,
                request.signed,
                request.clock.now().timestamp_millis(),
            )?;
            if state.roots.contains_key(&imported.root_id)
                || state.budget_parents.contains_key(&imported.root_id)
            {
                return Err(CoordinationError::Conflict);
            }
            state.roots.insert(
                imported.root_id.clone(),
                RootBudget {
                    owner_subject: imported.target.id().into(),
                    accepted_context: None,
                    limits: imported.limits.clone(),
                    spent_units: 0,
                    active_attempts: 0,
                    cancelled: false,
                },
            );
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            imported.authority = state.stamp();
            state
                .federation_imports
                .insert(imported.id.clone(), imported.clone());
            if state.record_count() > state.limits.max_records - CONTROL_RECORD_RESERVE
                || Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(imported);
            }
        }
        Err(CoordinationError::Contention)
    }

    pub(crate) fn check_federation_import_in_snapshot(
        state: &CoordinatorSnapshot,
        reference: &FederationImportReference,
        now_ms: i64,
    ) -> Result<FederationImportRecord, CoordinationError> {
        let imported = state
            .federation_imports
            .get(&reference.id)
            .filter(|record| record.envelope_digest == reference.envelope_digest)
            .ok_or(CoordinationError::Restricted)?;
        let trust = state
            .federation_trusts
            .get(&imported.trust_id)
            .filter(|record| {
                !record.revoked
                    && record.trust.revision == imported.trust_revision
                    && record.trust.foreign_domain == imported.issuer_domain
                    && imported.issuer_epoch >= record.trust.minimum_issuer_epoch
            })
            .ok_or(CoordinationError::Restricted)?;
        if now_ms < imported.accepted_at_ms
            || now_ms >= imported.expires_at_ms
            || now_ms >= trust.trust.deadline_ms
            || state.revoked_subjects.contains(imported.target.id())
            || !state.roots.get(&imported.root_id).is_some_and(|root| {
                root.owner_subject == imported.target.id()
                    && root.limits == imported.limits
                    && root.accepted_context.is_none()
                    && !root.cancelled
            })
        {
            return Err(CoordinationError::Restricted);
        }
        Ok(imported.clone())
    }

    pub(crate) fn validate_federated_attempt_in_snapshot(
        state: &CoordinatorSnapshot,
        check: Option<&FederationAttemptCheck<'_>>,
        subject: &str,
        resources: &BTreeSet<ResourceRef>,
        reservation: Option<&RootReservation>,
    ) -> Result<(), CoordinationError> {
        let uses_federated_root = reservation.is_some_and(|reservation| {
            state
                .federation_imports
                .values()
                .any(|import| import.root_id == reservation.root_id)
        });
        let Some(check) = check else {
            return if uses_federated_root {
                Err(CoordinationError::Restricted)
            } else {
                Ok(())
            };
        };
        let imported = Self::check_federation_import_in_snapshot(
            state,
            check.import,
            check.clock.now().timestamp_millis(),
        )?;
        if imported.target.id() != subject
            || !imported
                .effects
                .iter()
                .any(|allowed| matches_effect(allowed, check.effect))
            || resources != &check.effect.resources.iter().cloned().collect()
            || reservation.is_none_or(|reservation| reservation.root_id != imported.root_id)
        {
            return Err(CoordinationError::Restricted);
        }
        Ok(())
    }

    /// Recheck current local trust and expiry immediately before deriving local
    /// execution authority. This read alone does not register an effect.
    pub async fn check_federation_import(
        &self,
        reference: &FederationImportReference,
        clock: &dyn Clock,
    ) -> Result<FederationImportRecord, CoordinationError> {
        if !valid_text(&reference.id) || !digest_valid(&reference.envelope_digest, 32) {
            return Err(CoordinationError::Invalid(
                "federation import reference".into(),
            ));
        }
        let state = self.snapshot().await?;
        Self::check_federation_import_in_snapshot(&state, reference, clock.now().timestamp_millis())
    }

    /// Register an imported effect with current trust, exact attenuation and
    /// root accounting checked inside the same CAS as the effect start.
    pub async fn register_federated_attempt(
        &self,
        request: FederatedAttempt<'_>,
    ) -> Result<StartRegistration, CoordinationError> {
        if request.units == 0 {
            return Err(CoordinationError::Invalid("federated attempt units".into()));
        }
        if !valid_text(&request.import.id) || !digest_valid(&request.import.envelope_digest, 32) {
            return Err(CoordinationError::Invalid(
                "federation import reference".into(),
            ));
        }
        let snapshot = self.snapshot().await?;
        let imported = snapshot
            .federation_imports
            .get(&request.import.id)
            .filter(|record| record.envelope_digest == request.import.envelope_digest)
            .ok_or(CoordinationError::Restricted)?;
        self.register_attempt_checked(
            AttemptRequest {
                id: request.id,
                subject: imported.target.id(),
                resources: &request.effect.resources,
                request_digest: request.request_digest,
                expected_authority: request.expected_authority,
                reservation: Some(RootReservation {
                    root_id: imported.root_id.clone(),
                    units: request.units,
                }),
                now_ms: request.clock.now().timestamp_millis(),
            },
            None,
            None,
            Some(&FederationAttemptCheck {
                import: request.import,
                effect: request.effect,
                clock: request.clock,
            }),
        )
        .await
    }

    pub(crate) fn revoke_federation_trust(
        state: &mut CoordinatorSnapshot,
        trust_id: &str,
        expected_revision: u64,
    ) -> Result<(), CoordinationError> {
        let record = state
            .federation_trusts
            .get_mut(trust_id)
            .ok_or(CoordinationError::Conflict)?;
        if record.trust.revision != expected_revision {
            return Err(CoordinationError::Conflict);
        }
        record.revoked = true;
        Ok(())
    }
}
