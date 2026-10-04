//! Atomic, monotonic credential configuration for trusted host publication.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::credential::{CredentialAuthority, CredentialRecord};
use crate::permit::{
    PermitDenial, PermitIssuanceCeiling, matches_effect, valid_issuance, within_issuance,
};
use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE,
    CONTROL_RECORD_RESERVE, ChangeRecord, CoordinationError, RETRIES, valid_text,
};

const MAX_CREDENTIALS: usize = 128;

/// One complete scope projection of a trusted configuration source. The host
/// fingerprint must bind all security-relevant configuration, including roles
/// and credential rotation; use a keyed fingerprint when inputs contain secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialConfiguration {
    pub source_id: String,
    pub revision: u64,
    pub configuration_fingerprint: String,
    /// Every ceiling revision equals this configuration revision. An empty
    /// snapshot retires all credentials previously owned by this source.
    pub credentials: Vec<CredentialAuthority>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialConfigurationRecord {
    pub revision: u64,
    pub digest: String,
    /// Includes retired IDs: another source cannot adopt/reuse them.
    pub owned_credentials: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialConfigurationReference {
    pub source_id: String,
    pub revision: u64,
    pub digest: String,
    pub incarnation: String,
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
impl CredentialConfiguration {
    fn canonical(&self) -> Self {
        let mut value = self.clone();
        value
            .credentials
            .sort_by(|a, b| a.ceiling.id.cmp(&b.ceiling.id));
        for credential in &mut value.credentials {
            for effect in &mut credential.ceiling.effects {
                effect.resources.sort();
            }
            credential
                .ceiling
                .effects
                .sort_by(|a, b| (&a.operation, &a.resources).cmp(&(&b.operation, &b.resources)));
        }
        value
    }
    pub fn reference(
        &self,
        authority: &AuthorityStamp,
    ) -> Result<CredentialConfigurationReference, CoordinationError> {
        Ok(CredentialConfigurationReference {
            source_id: self.source_id.clone(),
            revision: self.revision,
            digest: digest(self)?,
            incarnation: authority.incarnation.clone(),
        })
    }
}
fn digest(configuration: &CredentialConfiguration) -> Result<String, CoordinationError> {
    let bytes = serde_json::to_vec(&configuration.canonical())
        .map_err(|_| CoordinationError::Invalid("configuration digest".into()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Shared by publication and history reconstruction. No writes happen here.
pub(crate) fn apply(
    coordinator: &AuthorityCoordinator,
    configuration: &CredentialConfiguration,
    credentials: &mut BTreeMap<String, CredentialRecord>,
    configurations: &mut BTreeMap<String, CredentialConfigurationRecord>,
) -> Result<(), CoordinationError> {
    if !coordinator.valid_credential_configuration(configuration) {
        return Err(CoordinationError::Invalid(
            "credential configuration".into(),
        ));
    }
    let previous = configurations.get(&configuration.source_id);
    if previous.is_some_and(|p| p.revision >= configuration.revision) {
        return Err(CoordinationError::StaleAuthority);
    }
    let old_ids = previous.map_or_else(BTreeSet::new, |p| p.owned_credentials.clone());
    let next_ids: BTreeSet<_> = configuration
        .credentials
        .iter()
        .map(|c| c.ceiling.id.clone())
        .collect();
    for credential in &configuration.credentials {
        let id = &credential.ceiling.id;
        if configurations.iter().any(|(source, record)| {
            source != &configuration.source_id && record.owned_credentials.contains(id)
        }) {
            return Err(CoordinationError::Conflict);
        }
        match credentials.get(id) {
            Some(old)
                if old_ids.contains(id)
                    && !old.revoked
                    && old.authority.ceiling.subject == credential.ceiling.subject
                    && old.authority.auth_method == credential.auth_method
                    && old.authority.ceiling.revision < credential.ceiling.revision => {}
            None if !old_ids.contains(id) => {}
            _ => return Err(CoordinationError::Conflict),
        }
    }
    for id in old_ids.difference(&next_ids) {
        credentials
            .get_mut(id)
            .ok_or(CoordinationError::Conflict)?
            .revoked = true;
    }
    for credential in &configuration.credentials {
        credentials.insert(
            credential.ceiling.id.clone(),
            CredentialRecord {
                authority: credential.clone(),
                revoked: false,
            },
        );
    }
    configurations.insert(
        configuration.source_id.clone(),
        CredentialConfigurationRecord {
            revision: configuration.revision,
            digest: digest(configuration)?,
            owned_credentials: old_ids.union(&next_ids).cloned().collect(),
        },
    );
    Ok(())
}

fn validate_ceiling(
    desired: &CredentialConfiguration,
    current: Option<&CredentialConfigurationRecord>,
    credentials: &BTreeMap<String, CredentialRecord>,
    issuance: &PermitIssuanceCeiling,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    if now_ms < issuance.valid_from_ms || now_ms >= issuance.limits.deadline_ms {
        return Err(CoordinationError::PermitDenied(PermitDenial::Validity));
    }
    for credential in &desired.credentials {
        let policy = &credential.ceiling;
        if !within_issuance(policy, issuance, now_ms) {
            return Err(CoordinationError::PermitDenied(PermitDenial::Effect));
        }
    }
    // Removal also requires publication authority over the previous
    // source's subjects/effects; an empty input cannot bypass the ceiling.
    if let Some(current) = current {
        for id in &current.owned_credentials {
            let previous = &credentials[id].authority.ceiling;
            if !issuance.subjects.contains(&previous.subject)
                || previous
                    .effects
                    .iter()
                    .any(|e| !issuance.effects.iter().any(|c| matches_effect(c, e)))
            {
                return Err(CoordinationError::PermitDenied(PermitDenial::Effect));
            }
        }
    }
    Ok(())
}

impl AuthorityCoordinator {
    pub(crate) fn valid_credential_configuration(
        &self,
        configuration: &CredentialConfiguration,
    ) -> bool {
        valid_text(&configuration.source_id)
            && configuration.revision > 0
            && valid_digest(&configuration.configuration_fingerprint)
            && configuration.credentials.len() <= MAX_CREDENTIALS
            && configuration
                .credentials
                .iter()
                .map(|c| &c.ceiling.id)
                .collect::<BTreeSet<_>>()
                .len()
                == configuration.credentials.len()
            && configuration
                .credentials
                .iter()
                .all(|c| c.ceiling.revision == configuration.revision && self.valid_credential(c))
    }

    /// Publish one scope atomically. Older configurations are rejected even if
    /// their earlier publication succeeded: an auth host must not install stale
    /// tables after observing a historical acknowledgment. Equal/current
    /// revisions observe the original event only when the full digest agrees.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_credential_configuration(
        &self,
        change_id: &str,
        configuration: &CredentialConfiguration,
        expected_revision: u64,
        issuance: &PermitIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        now_ms: i64,
    ) -> Result<ChangeRecord, CoordinationError> {
        if ![change_id, reason].into_iter().all(valid_text)
            || !self.valid_credential_configuration(configuration)
            || !valid_issuance(issuance)
            || now_ms < 0
        {
            return Err(CoordinationError::Invalid(
                "configuration publication".into(),
            ));
        }
        let desired = configuration.canonical();
        let desired_digest = digest(&desired)?;
        let change = AuthorityChange::PublishCredentialConfiguration {
            configuration: desired.clone(),
        };
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if evaluated_authority.incarnation != state.incarnation {
                return Err(CoordinationError::StaleAuthority);
            }
            let current = state.credential_configurations.get(&desired.source_id);
            if current.is_some_and(|r| r.revision > desired.revision) {
                return Err(CoordinationError::StaleAuthority);
            }
            if let Some(existing) = state.changes.get(change_id)
                && (existing.change != change
                    || existing.actor != issuance.issuer.id()
                    || existing.reason != reason)
            {
                return Err(CoordinationError::Conflict);
            }
            if let Some(current) = current
                && current.revision == desired.revision
            {
                if current.digest != desired_digest {
                    return Err(CoordinationError::Conflict);
                }
                return state
                    .changes
                    .values()
                    .find(|e| e.change == change)
                    .cloned()
                    .ok_or(CoordinationError::Conflict);
            }
            if current.map_or(0, |r| r.revision) != expected_revision
                || desired.revision <= expected_revision
            {
                return Err(CoordinationError::Conflict);
            }
            if state.stamp() != *evaluated_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if state.revoked_subjects.contains(issuance.issuer.id()) {
                return Err(CoordinationError::Restricted);
            }
            validate_ceiling(&desired, current, &state.credentials, issuance, now_ms)?;
            apply(
                self,
                &desired,
                &mut state.credentials,
                &mut state.credential_configurations,
            )?;
            if state.record_count() + 1 >= state.limits.max_records - CONTROL_RECORD_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            let event = ChangeRecord {
                change: change.clone(),
                actor: issuance.issuer.id().into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.changes.insert(change_id.into(), event.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(event);
            }
        }
        Err(CoordinationError::Contention)
    }

    /// Check a reference derived from the host's own trusted configuration and
    /// return the stamp from that same snapshot. This is freshness observation,
    /// not authentication, a bearer credential or execution authority.
    pub async fn verify_credential_configuration(
        &self,
        reference: &CredentialConfigurationReference,
    ) -> Result<AuthorityStamp, CoordinationError> {
        let state = self.snapshot().await?;
        if reference.incarnation != state.incarnation {
            return Err(CoordinationError::StaleAuthority);
        }
        let current = state
            .credential_configurations
            .get(&reference.source_id)
            .ok_or(CoordinationError::Conflict)?;
        if current.revision != reference.revision || current.digest != reference.digest {
            return Err(CoordinationError::StaleAuthority);
        }
        Ok(state.stamp())
    }
}
