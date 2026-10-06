//! Non-expiring first-admission identity with signed, bounded recovery data.
use acteon_state::{KeyKind, StateKey};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    ContextError, ContextRecord, FORMAT, MAX_BYTES, RootContextAdmission, SealedRecord,
    TrustedContextStore, VerifiedExecutionContext,
};
use crate::{RootBudgetLimits, credential::CredentialReference, permit::PermitReference};

pub const ROOT_ADMISSION_KIND: &str = "governance_root_admission";
const ADMISSION_FORMAT: u32 = 1;
const SIGNING_DOMAIN: &[u8] = b"acteon.root_admission.v1\0";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionRecord {
    format: u32,
    context: ContextRecord,
    limits: RootBudgetLimits,
}

pub struct IdempotentRootAdmission<'a> {
    pub admission_key: &'a str,
    pub admission: RootContextAdmission,
    pub permits: &'a [PermitReference],
    pub credential: CredentialReference,
    pub limits: RootBudgetLimits,
    pub clock: &'a dyn acteon_time::Clock,
}

impl TrustedContextStore {
    /// Pin first admission in the configured state backend before publishing
    /// its context and budget. A lost acknowledgement reuses the same identity
    /// and deadline. Recovery rechecks exact current permits and credentials;
    /// it never resets spending or establishes authority to send an effect.
    /// The key names one operation within this scope and signing domain.
    pub async fn capture_idempotent_credentialed_root(
        &self,
        admission_key: &str,
        admission: RootContextAdmission,
        permits: &[PermitReference],
        credential: CredentialReference,
        limits: RootBudgetLimits,
        clock: &dyn acteon_time::Clock,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        self.capture_idempotent_inner(
            IdempotentRootAdmission {
                admission_key,
                admission,
                permits,
                credential,
                limits,
                clock,
            },
            None,
            Vec::new(),
        )
        .await
    }
    pub async fn capture_idempotent_represented_credentialed_root(
        &self,
        request: IdempotentRootAdmission<'_>,
        representation: &crate::workforce::VerifiedRepresentation,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        self.capture_idempotent_inner(request, Some(representation), Vec::new())
            .await
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one recoverable immutable root acceptance protocol"
    )]
    pub(super) async fn capture_idempotent_inner(
        &self,
        request: IdempotentRootAdmission<'_>,
        representation: Option<&crate::workforce::VerifiedRepresentation>,
        delegation_grants: Vec<crate::delegation_policy::DelegationGrantReference>,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let IdempotentRootAdmission {
            admission_key,
            mut admission,
            permits,
            credential,
            limits,
            clock,
        } = request;
        if representation.is_some_and(|r| !r.binds(&admission, &limits)) {
            return Err(ContextError::Verification);
        }
        let representation = representation.map(|r| r.binding.clone());
        if admission_key.is_empty()
            || admission_key.len() > 512
            || admission_key.trim() != admission_key
            || admission_key.chars().any(char::is_control)
        {
            return Err(ContextError::Invalid);
        }
        admission.accepted_ceiling_revision = crate::permit::permit_revision_tag(permits)?;
        let now_ms = clock.now().timestamp_millis();
        let state = self.coordinator.snapshot().await?;
        let mut proposed = self.admission_record(
            &admission,
            credential.clone(),
            limits,
            now_ms,
            representation,
        );
        proposed.context.delegation_grants = delegation_grants;
        self.validate(&proposed.context)?;
        let (key, key_id) = self.root_admission_key(admission_key)?;
        let encoded = self.seal_admission(&proposed, &key_id)?;
        let stored = if let Some(value) = self.store.get(&key).await? {
            value
        } else {
            super::delegated::validate_grants(
                &state,
                &admission.binding.principal,
                &admission.accepted_effects,
                &proposed.context.delegation_grants,
                None,
                now_ms,
            )?;
            crate::permit::validate_root_admission_represented(
                &state,
                &admission,
                permits,
                &proposed.limits,
                now_ms,
                proposed.context.representation.as_ref(),
            )?;
            crate::credential::validate_root(
                &state,
                &admission,
                &credential,
                &proposed.limits,
                now_ms,
            )?;
            if self.store.check_and_set(&key, &encoded, None).await? {
                encoded
            } else {
                self.store.get(&key).await?.ok_or(ContextError::Missing)?
            }
        };
        let original = self.open_admission(&stored, &key_id)?;
        Self::match_admission(&original, &proposed)?;
        let record = &original.context;
        if now_ms < record.admitted_at_ms || now_ms >= record.deadline_ms {
            return Err(ContextError::Expired);
        }
        if record.authority.incarnation != state.incarnation {
            return Err(ContextError::Incarnation);
        }
        // Use the original ceilings, not the retry's newly computed deadline.
        admission.handle = record.handle.clone();
        admission.binding.execution_id = record.execution_id;
        admission.deadline_ms = record.deadline_ms;
        crate::permit::validate_root_admission_represented(
            &state,
            &admission,
            permits,
            &original.limits,
            now_ms,
            original.context.representation.as_ref(),
        )?;
        crate::credential::validate_root(
            &state,
            &admission,
            &credential,
            &original.limits,
            now_ms,
        )?;
        super::delegated::validate_grants(
            &state,
            &record.principal,
            &record.accepted_effects,
            &record.delegation_grants,
            None,
            now_ms,
        )?;
        self.restore_admitted_context(admission, record, credential, original.limits, clock)
            .await
    }
    async fn restore_admitted_context(
        &self,
        admission: RootContextAdmission,
        record: &ContextRecord,
        credential: CredentialReference,
        limits: RootBudgetLimits,
        clock: &dyn acteon_time::Clock,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let context = match self
            .recover(
                &record.handle,
                &admission.binding,
                clock.now().timestamp_millis(),
            )
            .await
        {
            Ok(existing) => {
                // Authority generation can change; every other original fact is immutable.
                let mut expected = record.clone();
                expected.authority = existing.0.authority.clone();
                Self::check_replay(existing, &expected)?
            }
            Err(ContextError::Missing) => {
                self.capture_root_inner(
                    admission.clone(),
                    record.admitted_at_ms,
                    Some(credential),
                    record.representation.clone(),
                    record.delegation_grants.clone(),
                )
                .await?
            }
            Err(error) => return Err(error),
        };
        self.coordinator
            .create_root_budget(
                &context.execution_id().to_string(),
                context.principal().id(),
                limits,
                &admission.evaluated_authority,
                clock.now().timestamp_millis(),
            )
            .await?;
        Ok(context)
    }

    fn admission_record(
        &self,
        admission: &RootContextAdmission,
        credential: CredentialReference,
        limits: RootBudgetLimits,
        now_ms: i64,
        representation: Option<crate::workforce::RepresentationBinding>,
    ) -> AdmissionRecord {
        AdmissionRecord {
            format: ADMISSION_FORMAT,
            context: ContextRecord {
                schema_version: FORMAT,
                domain: self.domain.clone(),
                namespace: self.coordinator.key.namespace.as_str().into(),
                tenant: self.coordinator.key.tenant.as_str().into(),
                handle: admission.handle.clone(),
                execution_id: admission.binding.execution_id,
                principal: admission.binding.principal.clone(),
                credential_id: admission.credential_id.clone(),
                auth_method: admission.auth_method.clone(),
                credential_authority: Some(credential),
                representation,
                lineage: None,
                delegation_grants: Vec::new(),
                delegated_from: None,
                request_digest: admission.binding.request_digest.clone(),
                accepted_ceiling_revision: admission.accepted_ceiling_revision.clone(),
                accepted_effects: admission.accepted_effects.clone(),
                deadline_ms: limits.deadline_ms,
                admitted_at_ms: now_ms,
                authority: admission.evaluated_authority.clone(),
            },
            limits,
        }
    }

    fn root_admission_key(&self, admission_key: &str) -> Result<(StateKey, String), ContextError> {
        let bytes = serde_json::to_vec(&(self.domain.as_str(), admission_key))
            .map_err(|_| ContextError::Invalid)?;
        let key_id = format!("{:x}", Sha256::digest(bytes));
        let key = StateKey::new(
            self.coordinator.key.namespace.as_str(),
            self.coordinator.key.tenant.as_str(),
            KeyKind::Custom(ROOT_ADMISSION_KIND.into()),
            &key_id,
        );
        Ok((key, key_id))
    }

    fn match_admission(
        original: &AdmissionRecord,
        proposed: &AdmissionRecord,
    ) -> Result<(), ContextError> {
        let mut expected = original.context.clone();
        // Candidate allocation and clock are irrelevant after first acceptance.
        expected.handle = proposed.context.handle.clone();
        expected.execution_id = proposed.context.execution_id;
        expected.admitted_at_ms = proposed.context.admitted_at_ms;
        expected.deadline_ms = proposed.context.deadline_ms;
        expected.authority = proposed.context.authority.clone();
        if expected.lineage.is_none()
            && proposed.context.lineage.is_none()
            && (expected.schema_version != 2 || proposed.context.representation.is_none())
            && (proposed.context.schema_version != 2 || expected.representation.is_none())
            && matches!(expected.schema_version, 2..=5)
            && matches!(proposed.context.schema_version, 2..=5)
        {
            expected.schema_version = proposed.context.schema_version;
        }
        if expected != proposed.context
            || original.limits.max_units != proposed.limits.max_units
            || original.limits.max_concurrent != proposed.limits.max_concurrent
        {
            return Err(ContextError::Conflict);
        }
        Ok(())
    }

    fn seal_admission(
        &self,
        record: &AdmissionRecord,
        key_id: &str,
    ) -> Result<String, ContextError> {
        let payload = serde_json::to_string(record).map_err(|_| ContextError::Invalid)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(
            self.keys
                .get(&self.active_key)
                .ok_or(ContextError::Invalid)?,
        )
        .map_err(|_| ContextError::Invalid)?;
        mac.update(SIGNING_DOMAIN);
        mac.update(key_id.as_bytes());
        mac.update(payload.as_bytes());
        let encoded = serde_json::to_string(&SealedRecord {
            schema_version: ADMISSION_FORMAT,
            key_id: self.active_key.clone(),
            payload,
            tag: mac.finalize().into_bytes().to_vec(),
        })
        .map_err(|_| ContextError::Invalid)?;
        if encoded.len() > MAX_BYTES {
            return Err(ContextError::Invalid);
        }
        Ok(encoded)
    }

    fn open_admission(&self, encoded: &str, key_id: &str) -> Result<AdmissionRecord, ContextError> {
        if encoded.len() > MAX_BYTES {
            return Err(ContextError::Verification);
        }
        let sealed: SealedRecord =
            serde_json::from_str(encoded).map_err(|_| ContextError::Verification)?;
        if sealed.schema_version != ADMISSION_FORMAT || sealed.tag.len() != 32 {
            return Err(ContextError::Verification);
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(
            self.keys
                .get(&sealed.key_id)
                .ok_or(ContextError::Verification)?,
        )
        .map_err(|_| ContextError::Verification)?;
        mac.update(SIGNING_DOMAIN);
        mac.update(key_id.as_bytes());
        mac.update(sealed.payload.as_bytes());
        mac.verify_slice(&sealed.tag)
            .map_err(|_| ContextError::Verification)?;
        let record: AdmissionRecord =
            serde_json::from_str(&sealed.payload).map_err(|_| ContextError::Verification)?;
        self.validate(&record.context)?;
        if record.format != ADMISSION_FORMAT
            || record.context.credential_authority.is_none()
            || record.limits.max_units == 0
            || record.limits.max_concurrent == 0
            || record.limits.deadline_ms != record.context.deadline_ms
        {
            return Err(ContextError::Verification);
        }
        Ok(record)
    }
}
