//! Shared root accounting at the same CAS boundary as effect starts.
use serde::{Deserialize, Serialize};

use crate::{
    AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE, CONTROL_RECORD_RESERVE,
    CoordinationError, RETRIES, valid_text,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetLimits {
    pub max_units: u64,
    pub max_concurrent: u64,
    pub deadline_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudget {
    /// Root owner is checked as an additional revocation ancestor.
    pub owner_subject: String,
    pub limits: RootBudgetLimits,
    /// Units are spent once per registered attempt, including retries. Known
    /// settlement releases concurrency, not spent call units.
    pub spent_units: u64,
    pub active_attempts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootReservation {
    pub root_id: String,
    pub units: u64,
}

impl AuthorityCoordinator {
    /// Trusted adapter creates a bounded root allocation from independently
    /// evaluated permits. Replay observes it; it never increases its limits.
    /// This is not aggregate team funding or authorization of delegated lineage.
    pub async fn create_root_budget(
        &self,
        root_id: &str,
        owner_subject: &str,
        limits: RootBudgetLimits,
        evaluated_authority: &AuthorityStamp,
        now_ms: i64,
    ) -> Result<RootBudget, CoordinationError> {
        if !valid_text(root_id)
            || !valid_text(owner_subject)
            || limits.max_units == 0
            || limits.max_concurrent == 0
            || limits.deadline_ms <= 0
            || now_ms < 0
        {
            return Err(CoordinationError::Invalid("root budget fields".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if evaluated_authority.incarnation != state.incarnation {
                return Err(CoordinationError::StaleAuthority);
            }
            if let Some(existing) = state.roots.get(root_id) {
                if existing.owner_subject != owner_subject || existing.limits != limits {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(existing.clone());
            }
            if state.stamp() != *evaluated_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if state.revoked_subjects.contains(owner_subject) {
                return Err(CoordinationError::Restricted);
            }
            if limits.deadline_ms <= now_ms {
                return Err(CoordinationError::DeadlineExceeded);
            }
            if limits.max_concurrent > u64::try_from(state.limits.max_active).unwrap_or(u64::MAX)
                || state.record_count() >= state.limits.max_records - CONTROL_RECORD_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            let root = RootBudget {
                owner_subject: owner_subject.into(),
                limits: limits.clone(),
                spent_units: 0,
                active_attempts: 0,
            };
            state.roots.insert(root_id.into(), root.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(root);
            }
        }
        Err(CoordinationError::Contention)
    }
}

// The caller commits this mutation together with the new start record.
pub(crate) fn reserve_root(
    state: &mut crate::CoordinatorSnapshot,
    reservation: &RootReservation,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    let root = state
        .roots
        .get_mut(&reservation.root_id)
        .ok_or(CoordinationError::Conflict)?;
    if state.revoked_subjects.contains(&root.owner_subject) {
        return Err(CoordinationError::Restricted);
    }
    if now_ms >= root.limits.deadline_ms {
        return Err(CoordinationError::DeadlineExceeded);
    }
    if root.active_attempts >= root.limits.max_concurrent {
        return Err(CoordinationError::ConcurrencyExhausted);
    }
    let spent = root
        .spent_units
        .checked_add(reservation.units)
        .filter(|spent| *spent <= root.limits.max_units)
        .ok_or(CoordinationError::BudgetExhausted)?;
    root.spent_units = spent;
    root.active_attempts += 1;
    Ok(())
}
