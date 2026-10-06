//! Permanent scope ownership at the same CAS boundary as authority changes.
use serde::{Deserialize, Serialize};

use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, CoordinationError, CoordinatorSnapshot,
    RETRIES, valid_text,
};

/// Inspection metadata, not a management capability. A trusted host must
/// reserve a virgin scope before installing its execution or auth publisher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopePurpose {
    Unclaimed,
    AuthenticationControl { source_id: String },
    Execution,
}

impl ScopePurpose {
    pub(crate) fn valid(&self) -> bool {
        match self {
            Self::AuthenticationControl { source_id } => {
                valid_text(source_id) && source_id.len() <= 512 && source_id.trim() == source_id
            }
            Self::Unclaimed | Self::Execution => true,
        }
    }
}

impl AuthorityCoordinator {
    /// Reserve once on a virgin coordinator; replay observes the same owner.
    /// There is no release or reassignment operation. Existing unclaimed work
    /// requires an explicit migration, never automatic adoption at startup.
    pub async fn reserve_scope(
        &self,
        purpose: ScopePurpose,
    ) -> Result<AuthorityStamp, CoordinationError> {
        if !purpose.valid() || purpose == ScopePurpose::Unclaimed {
            return Err(CoordinationError::Invalid("scope purpose".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if state.purpose == purpose {
                return Ok(state.stamp());
            }
            if state.purpose != ScopePurpose::Unclaimed
                || state.record_count() != 0
                || !state.closed_resources.is_empty()
                || !state.revoked_subjects.is_empty()
                || state.generation != 1
            {
                return Err(CoordinationError::Conflict);
            }
            state.purpose = purpose.clone();
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            state.changes.insert(
                "scope-purpose".into(),
                crate::ChangeRecord {
                    change: AuthorityChange::ReserveScope {
                        purpose: purpose.clone(),
                    },
                    actor: "acteon.scope.bootstrap".into(),
                    reason: "trusted host permanent scope reservation".into(),
                    generation: state.generation,
                    pending: true,
                },
            );
            if self.commit(&state, version).await? {
                return Ok(state.stamp());
            }
        }
        Err(CoordinationError::Contention)
    }

    pub(crate) fn valid_scope_purpose(state: &CoordinatorSnapshot) -> bool {
        if !state.purpose.valid() {
            return false;
        }
        let reservations: Vec<_> = state
            .changes
            .iter()
            .filter_map(|(id, record)| {
                if let AuthorityChange::ReserveScope { purpose } = &record.change {
                    Some((id, record, purpose))
                } else {
                    None
                }
            })
            .collect();
        if state.purpose == ScopePurpose::Unclaimed {
            if !reservations.is_empty() {
                return false;
            }
        } else if reservations.len() != 1
            || reservations[0].0 != "scope-purpose"
            || reservations[0].1.generation < 2
            || reservations[0].2 != &state.purpose
        {
            return false;
        }
        match &state.purpose {
            ScopePurpose::Unclaimed | ScopePurpose::Execution => true,
            ScopePurpose::AuthenticationControl { source_id } => {
                state.budget_parents.is_empty()
                    && state.workforce.record_count() == 0
                    && state.starts.is_empty()
                    && state.roots.is_empty()
                    && state.permits.is_empty()
                    && state.credentials.is_empty()
                    && state.closed_resources.is_empty()
                    && state
                        .credential_configurations
                        .iter()
                        .all(|(id, record)| id == source_id && record.owned_credentials.is_empty())
                    && state.changes.values().all(|record| match &record.change {
                        AuthorityChange::PublishCredentialConfiguration { configuration } => {
                            configuration.source_id == *source_id
                                && configuration.credentials.is_empty()
                        }
                        AuthorityChange::ReserveScope { purpose } => purpose == &state.purpose,
                        AuthorityChange::RevokeSubject { .. }
                        | AuthorityChange::UpgradeProtocol { .. } => true,
                        _ => false,
                    })
            }
        }
    }
}
